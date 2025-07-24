use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePath {
    path: Vec<u8>,
}

impl StorePath {
    pub fn new(path: Vec<u8>) -> Self {
        Self { path }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.path
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.path))
    }
}

impl From<String> for StorePath {
    fn from(path: String) -> Self {
        Self::new(path.into_bytes())
    }
}

impl From<&str> for StorePath {
    fn from(path: &str) -> Self {
        Self::new(path.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for StorePath {
    fn from(path: Vec<u8>) -> Self {
        Self::new(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPathInfo {
    pub deriver: Option<StorePath>,
    pub hash: Vec<u8>,
    pub references: Vec<StorePath>,
    pub registration_time: u64,
    pub nar_size: u64,
    pub ultimate: bool,
    pub signatures: Vec<Vec<u8>>,
    pub content_address: Option<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Msg {
    Write = 0x64617416,
    Error = 0x63787470,
    Next = 0x6f6c6d67,
    StartActivity = 0x53545254,
    StopActivity = 0x53544f50,
    Result = 0x52534c54,
    Last = 0x616c7473,
}

impl TryFrom<u64> for Msg {
    type Error = crate::error::ProtocolError;

    fn try_from(value: u64) -> Result<Self, crate::error::ProtocolError> {
        match value {
            0x64617416 => Ok(Self::Write),
            0x63787470 => Ok(Self::Error),
            0x6f6c6d67 => Ok(Self::Next),
            0x53545254 => Ok(Self::StartActivity),
            0x53544f50 => Ok(Self::StopActivity),
            0x52534c54 => Ok(Self::Result),
            0x616c7473 => Ok(Self::Last),
            _ => Err(crate::error::ProtocolError::InvalidMsgCode(value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StderrError {
    pub typ: String,
    pub level: u64,
    pub name: String,
    pub message: String,
    pub have_pos: u64,
    pub traces: Vec<Trace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    pub have_pos: u64,
    pub trace: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoggerField {
    Int(u64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StderrStartActivity {
    pub act: u64,
    pub lvl: u64,
    pub typ: u64,
    pub s: String,
    pub fields: LoggerField,
    pub parent: u64,
}
