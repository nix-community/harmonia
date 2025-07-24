use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProtocolVersion {
    pub major: u8,
    pub minor: u8,
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl From<u64> for ProtocolVersion {
    fn from(x: u64) -> Self {
        let major = ((x >> 8) & 0xff) as u8;
        let minor = (x & 0xff) as u8;
        Self { major, minor }
    }
}

impl From<ProtocolVersion> for u64 {
    fn from(version: ProtocolVersion) -> Self {
        ((version.major as u64) << 8) | version.minor as u64
    }
}

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 38,
};
pub const MIN_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 21,
};
