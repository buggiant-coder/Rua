use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlcTimestamp {
    pub wall_ms: u64,
    pub counter: u32,
    pub device_id: [u8; 8],
}

impl HlcTimestamp {
    pub fn to_sortable_bytes(&self) -> [u8; 20] {
        let mut buf = [0u8; 20];
        buf[0..8].copy_from_slice(&self.wall_ms.to_be_bytes());
        buf[8..12].copy_from_slice(&self.counter.to_be_bytes());
        buf[12..20].copy_from_slice(&self.device_id);
        buf
    }

    pub fn from_sortable_bytes(bytes: &[u8; 20]) -> Self {
        let wall_ms = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
        let counter = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        let mut device_id = [0u8; 8];
        device_id.copy_from_slice(&bytes[12..20]);
        Self {
            wall_ms,
            counter,
            device_id,
        }
    }

    pub fn to_sortable_string(&self) -> String {
        hex::encode(self.to_sortable_bytes())
    }

    pub fn from_sortable_string(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        let arr: [u8; 20] = bytes
            .try_into()
            .map_err(|_| hex::FromHexError::InvalidStringLength)?;
        Ok(Self::from_sortable_bytes(&arr))
    }
}

impl PartialOrd for HlcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HlcTimestamp {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_sortable_bytes().cmp(&other.to_sortable_bytes())
    }
}

impl fmt::Display for HlcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sortable_string())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as u64
}

pub struct HlcClock {
    last: HlcTimestamp,
    device_id: [u8; 8],
}

impl HlcClock {
    pub fn new(device_id: [u8; 8]) -> Self {
        Self {
            last: HlcTimestamp {
                wall_ms: 0,
                counter: 0,
                device_id,
            },
            device_id,
        }
    }

    pub fn resume(device_id: [u8; 8], last: HlcTimestamp) -> Self {
        Self { last, device_id }
    }

    pub fn now(&mut self) -> HlcTimestamp {
        let wall = now_ms();
        let ts = if wall > self.last.wall_ms {
            HlcTimestamp {
                wall_ms: wall,
                counter: 0,
                device_id: self.device_id,
            }
        } else {
            HlcTimestamp {
                wall_ms: self.last.wall_ms,
                counter: self.last.counter + 1,
                device_id: self.device_id,
            }
        };
        self.last = ts;
        ts
    }

    pub fn last(&self) -> &HlcTimestamp {
        &self.last
    }
}
