#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStatus {
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelInfo {
    pub public_url: String,
    pub public_wss_url: String,
    pub local_port: u16,
    pub status: TunnelStatus,
}

impl TunnelInfo {
    pub fn new(public_url: String, local_port: u16) -> Self {
        let public_wss_url = public_url.replace("https://", "wss://");
        Self {
            public_url,
            public_wss_url,
            local_port,
            status: TunnelStatus::Running,
        }
    }
}
