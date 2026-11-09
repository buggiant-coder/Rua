//! Read counters for one known socket in this process. Never read packet payloads.

use std::net::SocketAddr;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SocketCounters {
    pub tx_bytes: u64,
    pub tx_retransmit_bytes: u64,
    pub rx_bytes: u64,
    pub rx_out_of_order_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_retransmit_packets: Option<u64>,
    pub send_queue_bytes: u32,
    pub receive_queue_bytes: Option<u32>,
    pub rtt_ms: u32,
    pub retransmit_timeout_ms: u32,
    pub send_window_bytes: u32,
    pub congestion_window_bytes: u32,
    pub tcp_state: u8,
}

#[derive(Debug, Serialize)]
pub struct SocketReading {
    pub status: &'static str,
    pub counters: Option<SocketCounters>,
}

#[derive(Default)]
pub struct SocketSampler {
    #[cfg(target_os = "macos")]
    cached_fd: Option<i32>,
}

pub const SOCKET_COUNTERS_SUPPORTED: bool = cfg!(target_os = "macos");

impl SocketSampler {
    pub fn sample(&mut self, local: SocketAddr, remote: SocketAddr) -> SocketReading {
        #[cfg(target_os = "macos")]
        let result = macos::sample(&mut self.cached_fd, local, remote);
        #[cfg(not(target_os = "macos"))]
        let result = {
            let _ = (local, remote);
            Err("unsupported_platform")
        };
        match result {
            Ok(counters) => SocketReading {
                status: "observed",
                counters: Some(counters),
            },
            Err(reason) => SocketReading {
                status: reason,
                counters: None,
            },
        }
    }
}

/// A non-adjustable, suspend-inclusive clock, or None on unsupported platforms.
/// Only compare differences from this clock, never its origin with another clock.
pub fn suspend_inclusive_time() -> Option<std::time::Duration> {
    #[cfg(target_os = "macos")]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: time is writable and correctly sized for clock_gettime.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut time) } != 0 {
            return None;
        }
        Some(std::time::Duration::new(
            u64::try_from(time.tv_sec).ok()?,
            u32::try_from(time.tv_nsec).ok()?,
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::mem::{size_of, zeroed};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    use super::SocketCounters;

    const MAX_FDS: usize = 8192;

    // libc 0.2's Apple tcp_connection_info expands C bitfields into separate
    // u32s, so its counter offsets do not match the SDK. Keep the SDK's single
    // packed TFO flags word. Validate the ABI with real loopback transfers.
    #[repr(C)]
    struct TcpInfo {
        state: u8,
        _snd_scale: u8,
        _rcv_scale: u8,
        _pad: u8,
        _options: u32,
        _flags: u32,
        rto: u32,
        _maxseg: u32,
        _ssthresh: u32,
        cwnd: u32,
        send_window: u32,
        send_queue: u32,
        _receive_window: u32,
        _rtt_current: u32,
        rtt: u32,
        _rtt_variance: u32,
        _tfo_flags: u32,
        tx_packets: u64,
        tx_bytes: u64,
        tx_retransmit_bytes: u64,
        rx_packets: u64,
        rx_bytes: u64,
        rx_out_of_order_bytes: u64,
        tx_retransmit_packets: u64,
    }

    const _: () = assert!(size_of::<TcpInfo>() == 112);
    const _: () = assert!(std::mem::offset_of!(TcpInfo, tx_bytes) == 64);

    pub(super) fn sample(
        cached_fd: &mut Option<i32>,
        local: SocketAddr,
        remote: SocketAddr,
    ) -> Result<SocketCounters, &'static str> {
        if let Some(result) = cached_fd.and_then(|fd| read_fd(fd, local, remote)) {
            return result;
        }
        *cached_fd = None;
        let mut fds = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0
            };
            MAX_FDS
        ];
        let capacity = (fds.len() * size_of::<libc::proc_fdinfo>()) as i32;
        // SAFETY: the buffer is initialized, aligned, and capacity is its byte size.
        let length = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                capacity,
            )
        };
        if length <= 0 || length > capacity {
            return Err("fd_enumeration_unavailable");
        }
        for fd in fds
            .iter()
            .take(length as usize / size_of::<libc::proc_fdinfo>())
        {
            if fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32 {
                if let Some(result) = read_fd(fd.proc_fd, local, remote) {
                    *cached_fd = Some(fd.proc_fd);
                    return result;
                }
            }
        }
        Err(if length == capacity {
            "fd_scan_limit"
        } else {
            "socket_not_found"
        })
    }

    fn read_fd(
        fd: i32,
        local: SocketAddr,
        remote: SocketAddr,
    ) -> Option<Result<SocketCounters, &'static str>> {
        // Pin the descriptor only for this read. Do not keep pooled sockets alive
        // between samples, and never operate on an fd that was closed/reused.
        // SAFETY: fcntl validates fd; a successful duplicate is uniquely owned here.
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return None;
        }
        // SAFETY: duplicate is a newly owned valid fd returned above.
        let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
        let fd = duplicate.as_raw_fd();
        if address(fd, false)? != local || address(fd, true)? != remote {
            return None;
        }
        // SAFETY: all-zero TcpInfo is valid; getsockopt receives the full buffer size.
        let mut info: TcpInfo = unsafe { zeroed() };
        let mut length = size_of::<TcpInfo>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::IPPROTO_TCP,
                libc::TCP_CONNECTION_INFO,
                (&mut info as *mut TcpInfo).cast(),
                &mut length,
            )
        };
        // Older kernels may omit the final retransmit-packet counter.
        if result != 0 || length < 104 {
            return Some(Err("tcp_info_unavailable"));
        }
        let mut readable: libc::c_int = 0;
        // SAFETY: FIONREAD writes an int and does not consume socket data.
        let receive_queue = (unsafe { libc::ioctl(fd, libc::FIONREAD, &mut readable) } == 0)
            .then(|| u32::try_from(readable).ok())
            .flatten();
        Some(Ok(SocketCounters {
            tx_bytes: info.tx_bytes,
            tx_retransmit_bytes: info.tx_retransmit_bytes,
            rx_bytes: info.rx_bytes,
            rx_out_of_order_bytes: info.rx_out_of_order_bytes,
            tx_packets: info.tx_packets,
            rx_packets: info.rx_packets,
            tx_retransmit_packets: (length as usize >= size_of::<TcpInfo>())
                .then_some(info.tx_retransmit_packets),
            send_queue_bytes: info.send_queue,
            receive_queue_bytes: receive_queue,
            rtt_ms: info.rtt,
            retransmit_timeout_ms: info.rto,
            send_window_bytes: info.send_window,
            congestion_window_bytes: info.cwnd,
            tcp_state: info.state,
        }))
    }

    fn address(fd: i32, peer: bool) -> Option<SocketAddr> {
        // SAFETY: sockaddr_storage accepts any socket address and is fully initialized.
        let mut address: libc::sockaddr_storage = unsafe { zeroed() };
        let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let result = unsafe {
            let pointer = (&mut address as *mut libc::sockaddr_storage).cast();
            if peer {
                libc::getpeername(fd, pointer, &mut length)
            } else {
                libc::getsockname(fd, pointer, &mut length)
            }
        };
        if result != 0 {
            return None;
        }
        match i32::from(address.ss_family) {
            libc::AF_INET if length as usize >= size_of::<libc::sockaddr_in>() => {
                // SAFETY: family, size, and storage alignment match sockaddr_in.
                let address = unsafe {
                    &*((&address as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>())
                };
                Some(SocketAddr::new(
                    Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()).into(),
                    u16::from_be(address.sin_port),
                ))
            }
            libc::AF_INET6 if length as usize >= size_of::<libc::sockaddr_in6>() => {
                // SAFETY: family, size, and storage alignment match sockaddr_in6.
                let address = unsafe {
                    &*((&address as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>())
                };
                Some(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(address.sin6_addr.s6_addr),
                    u16::from_be(address.sin6_port),
                    u32::from_be(address.sin6_flowinfo),
                    address.sin6_scope_id,
                )))
            }
            _ => None,
        }
    }
}
