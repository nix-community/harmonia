use std::fmt;

use anyhow::{bail, Context, Result};
use std::str;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

const SOCKET_PATH: &str = "/nix/var/nix/daemon-socket/socket";

const MAX_STRING_SIZE: u64 = 0x1000000; // 16M
const MAX_STRING_LIST_SIZE: u64 = 0x10000; // 64K

#[derive(Debug, Default)]
pub(crate) struct DaemonConnection {
    socket: Option<UnixStream>,
    #[allow(dead_code)]
    server_features: Vec<String>,
    #[allow(dead_code)]
    daemon_version: String,
    #[allow(dead_code)]
    is_trusted: bool,
}

const WORKER_MAGIC_1: u64 = 0x6e697863;
const WORKER_MAGIC_2: u64 = 0x6478696f;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProtocolVersion {
    major: u8,
    minor: u8,
}

impl From<u64> for ProtocolVersion {
    fn from(x: u64) -> Self {
        let major = ((x >> 8) & 0xff) as u8;
        let minor = (x & 0xff) as u8;
        Self { major, minor }
    }
}

impl From<ProtocolVersion> for u64 {
    fn from(ProtocolVersion { major, minor }: ProtocolVersion) -> Self {
        ((major as u64) << 8) | minor as u64
    }
}

const MINIMUM_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 21,
};

const CLIENT_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 38,
};

enum OpCode {
    IsValidPath = 1,
    HasSubstitutes = 3,
    QueryPathHash = 4,   // obsolete
    QueryReferences = 5, // obsolete
    QueryReferrers = 6,
    AddToStore = 7,
    AddTextToStore = 8, // obsolete since 1.25, Nix 3.0. Use WorkerProto::Op::AddToStore
    BuildPaths = 9,
    EnsurePath = 10,
    AddTempRoot = 11,
    AddIndirectRoot = 12,
    SyncWithGC = 13,
    FindRoots = 14,
    ExportPath = 16,   // obsolete
    QueryDeriver = 18, // obsolete
    SetOptions = 19,
    CollectGarbage = 20,
    QuerySubstitutablePathInfo = 21,
    QueryDerivationOutputs = 22, // obsolete
    QueryAllValidPaths = 23,
    QueryFailedPaths = 24,
    ClearFailedPaths = 25,
    QueryPathInfo = 26,
    ImportPaths = 27,                // obsolete
    QueryDerivationOutputNames = 28, // obsolete
    QueryPathFromHashPart = 29,
    QuerySubstitutablePathInfos = 30,
    QueryValidPaths = 31,
    QuerySubstitutablePaths = 32,
    QueryValidDerivers = 33,
    OptimiseStore = 34,
    VerifyStore = 35,
    BuildDerivation = 36,
    AddSignatures = 37,
    NarFromPath = 38,
    AddToStoreNar = 39,
    QueryMissing = 40,
    QueryDerivationOutputMap = 41,
    RegisterDrvOutput = 42,
    QueryRealisation = 43,
    AddMultipleToStore = 44,
    AddBuildLog = 45,
    BuildPathsWithResults = 46,
    AddPermRoot = 47,
}

#[derive(Debug)]
struct OpCodeError {
    code: u64,
}

impl fmt::Display for OpCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid OpCode: {}", self.code)
    }
}

impl std::error::Error for OpCodeError {}

impl TryFrom<u64> for OpCode {
    type Error = OpCodeError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::IsValidPath),
            3 => Ok(Self::HasSubstitutes),
            4 => Ok(Self::QueryPathHash),
            5 => Ok(Self::QueryReferences),
            6 => Ok(Self::QueryReferrers),
            7 => Ok(Self::AddToStore),
            8 => Ok(Self::AddTextToStore),
            9 => Ok(Self::BuildPaths),
            10 => Ok(Self::EnsurePath),
            11 => Ok(Self::AddTempRoot),
            12 => Ok(Self::AddIndirectRoot),
            13 => Ok(Self::SyncWithGC),
            14 => Ok(Self::FindRoots),
            16 => Ok(Self::ExportPath),
            18 => Ok(Self::QueryDeriver),
            19 => Ok(Self::SetOptions),
            20 => Ok(Self::CollectGarbage),
            21 => Ok(Self::QuerySubstitutablePathInfo),
            22 => Ok(Self::QueryDerivationOutputs),
            23 => Ok(Self::QueryAllValidPaths),
            24 => Ok(Self::QueryFailedPaths),
            25 => Ok(Self::ClearFailedPaths),
            26 => Ok(Self::QueryPathInfo),
            27 => Ok(Self::ImportPaths),
            28 => Ok(Self::QueryDerivationOutputNames),
            29 => Ok(Self::QueryPathFromHashPart),
            30 => Ok(Self::QuerySubstitutablePathInfos),
            31 => Ok(Self::QueryValidPaths),
            32 => Ok(Self::QuerySubstitutablePaths),
            33 => Ok(Self::QueryValidDerivers),
            34 => Ok(Self::OptimiseStore),
            35 => Ok(Self::VerifyStore),
            36 => Ok(Self::BuildDerivation),
            37 => Ok(Self::AddSignatures),
            38 => Ok(Self::NarFromPath),
            39 => Ok(Self::AddToStoreNar),
            40 => Ok(Self::QueryMissing),
            41 => Ok(Self::QueryDerivationOutputMap),
            42 => Ok(Self::RegisterDrvOutput),
            43 => Ok(Self::QueryRealisation),
            44 => Ok(Self::AddMultipleToStore),
            45 => Ok(Self::AddBuildLog),
            46 => Ok(Self::BuildPathsWithResults),
            47 => Ok(Self::AddPermRoot),
            _ => Err(OpCodeError { code: value }),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct ValidPathInfo {
    pub deriver: String,
    pub hash: String,
    pub references: Vec<String>,
    pub registration_time: u64, // In seconds, since the epoch
    pub nar_size: u64,
    pub ultimate: bool,
    pub sigs: Vec<String>,
    pub content_address: Option<String>, // Can be empty
}

#[derive(Debug, PartialEq)]
pub(crate) struct QueryPathInfoResponse {
    pub path: Option<ValidPathInfo>,
}

#[derive(Debug, PartialEq)]
enum Msg {
    Write = 0x64617416,
    Error = 0x63787470,
    Next = 0x6f6c6d67,
    StartActivity = 0x53545254,
    StopActivity = 0x53544f50,
    Result = 0x52534c54,
    Last = 0x616c7473,
}

#[derive(Debug)]
struct MsgCodeError {
    code: u64,
}

impl fmt::Display for MsgCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid Message code: {}", self.code)
    }
}

impl std::error::Error for MsgCodeError {}

impl TryFrom<u64> for Msg {
    type Error = MsgCodeError;

    fn try_from(value: u64) -> Result<Self, MsgCodeError> {
        match value {
            0x64617416 => Ok(Self::Write),
            0x63787470 => Ok(Self::Error),
            0x6f6c6d67 => Ok(Self::Next),
            0x53545254 => Ok(Self::StartActivity),
            0x53544f50 => Ok(Self::StopActivity),
            0x52534c54 => Ok(Self::Result),
            0x616c7473 => Ok(Self::Last),
            _ => Err(MsgCodeError { code: value }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StderrError {
    typ: String,
    level: u64,
    name: String,
    message: String,
    have_pos: u64,
    traces: Vec<Trace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Trace {
    have_pos: u64,
    trace: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoggerField {
    Int(u64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StderrStartActivity {
    act: u64,
    lvl: u64,
    typ: u64,
    s: String,
    fields: LoggerField,
    parent: u64,
}

async fn write_num<T: Into<u64>>(socket: &mut UnixStream, num: T) -> Result<()> {
    let num = num.into();
    socket
        .write_all(&num.to_le_bytes())
        .await
        .context("Failed to write number")
}

async fn read_num<T: From<u64>>(socket: &mut UnixStream) -> Result<T> {
    let mut buf = [0; 8];
    socket
        .read_exact(&mut buf)
        .await
        .context("Failed to read number")?;
    Ok(T::from(u64::from_le_bytes(buf)))
}

async fn write_string(socket: &mut UnixStream, s: &str) -> Result<()> {
    write_num::<u64>(socket, s.len() as u64).await?;
    socket.write_all(s.as_bytes()).await?;
    let padding = [0; 8];
    let padding_size = (8 - s.len() % 8) % 8;
    if padding_size > 0 {
        socket.write_all(&padding[0..padding_size]).await?;
    }
    Ok(())
}

async fn read_string(socket: &mut UnixStream) -> Result<String> {
    let len = read_num::<u64>(socket)
        .await
        .context("Failed to read string length")?;
    if len > MAX_STRING_SIZE {
        bail!("String too long: {} > {}", len, MAX_STRING_SIZE);
    }
    let aligned_len = (len + 7) & !7; // Align to the next multiple of 8
    let mut buf = vec![0; aligned_len as usize];
    socket
        .read_exact(&mut buf)
        .await
        .context("Failed to read string")?;
    Ok(str::from_utf8(&buf[..len as usize])
        .context("Failed to parse string")?
        .to_owned())
}

async fn read_string_list(socket: &mut UnixStream) -> Result<Vec<String>> {
    let len = read_num::<u64>(socket).await?;
    if len > MAX_STRING_LIST_SIZE {
        bail!("String list too long: {} > {}", len, MAX_STRING_LIST_SIZE);
    }
    let mut res = Vec::with_capacity(len as usize);
    for _ in 0..len {
        res.push(read_string(socket).await?);
    }
    Ok(res)
}

async fn write_string_list(socket: &mut UnixStream, list: &[String]) -> Result<()> {
    write_num::<u64>(socket, list.len() as u64).await?;
    for s in list {
        write_string(socket, s).await?;
    }
    Ok(())
}

struct Handshake {
    server_features: Vec<String>,
    daemon_version: String,
    is_trusted: bool,
}

async fn handshake(socket: &mut UnixStream) -> Result<Handshake> {
    write_num(socket, WORKER_MAGIC_1)
        .await
        .context("Failed to write magic 1")?;
    let magic = read_num::<u64>(socket)
        .await
        .context("Failed to read magic 2")?;
    if magic != WORKER_MAGIC_2 {
        bail!("Invalid magic number: {}", magic);
    }
    let protocol_version = read_num::<u64>(socket)
        .await
        .context("Failed to read protocol version")?;
    if protocol_version < u64::from(MINIMUM_PROTOCOL_VERSION) {
        bail!("Protocol version mismatch: got {}", protocol_version);
    }

    write_num::<u64>(socket, CLIENT_VERSION.into())
        .await
        .context("Failed to write client version")?;
    write_num(socket, 0u64)
        .await
        .context("Failed to cpu affinity flags")?; // cpu affinity, obsolete
    write_num(socket, 0u64)
        .await
        .context("Failed to write flags")?; // reserve space, obsolete

    /* Exchange features. */
    let server_features = if protocol_version >= 0x126 {
        let features = read_string_list(socket)
            .await
            .context("Failed to read daemon features")?;
        write_string_list(socket, &[])
            .await
            .context("Failed to write supported features")?;
        features
    } else {
        Vec::new()
    };

    let daemon_version = read_string(socket)
        .await
        .context("Failed to read daemon version")?;

    let is_trusted = read_num::<u64>(socket)
        .await
        .context("Failed to read is_trusted")?
        == 1;

    forward_stderr(socket).await?;

    Ok(Handshake {
        server_features,
        daemon_version,
        is_trusted,
    })
}

async fn forward_stderr(socket: &mut UnixStream) -> Result<()> {
    loop {
        let msg_code = read_num::<u64>(socket).await?;
        let msg = Msg::try_from(msg_code)?;
        match msg {
            Msg::Error => {
                let mut err = StderrError {
                    typ: read_string(socket).await.context("Failed to read type")?,
                    level: read_num(socket).await.context("Failed to read level")?,
                    name: read_string(socket).await.context("Failed to read name")?,
                    message: read_string(socket)
                        .await
                        .context("Failed to read message")?,
                    have_pos: read_num(socket).await.context("Failed to read have_pos")?,
                    traces: Vec::new(),
                };
                let traces_len = read_num::<u64>(socket)
                    .await
                    .context("Failed to read traces_len")?;
                for _ in 0..traces_len {
                    err.traces.push(Trace {
                        have_pos: read_num(socket).await.context("Failed to read have_pos")?,
                        trace: read_string(socket).await.context("Failed to read trace")?,
                    });
                }
                bail!("Daemon error: {}", err.message);
            }
            Msg::Next => {
                let next = read_string(socket).await.context("Failed to read next")?;
                eprintln!("[nix-daemon]: {}", next);
            }
            Msg::StartActivity => {
                let act = read_num(socket).await.context("Failed to read act")?;
                let lvl = read_num(socket).await.context("Failed to read lvl")?;
                let typ = read_num(socket).await.context("Failed to read typ")?;
                let s = read_string(socket).await.context("Failed to read s")?;
                let fields = match read_num::<u64>(socket)
                    .await
                    .context("Failed to read fields")?
                {
                    0 => LoggerField::Int(read_num(socket).await.context("Failed to read int")?),
                    1 => LoggerField::String(
                        read_string(socket).await.context("Failed to read string")?,
                    ),
                    _ => bail!("Invalid field type"),
                };
                let parent = read_num(socket).await.context("Failed to read parent")?;
                eprintln!(
                    "[nix-daemon] start activity: {:?}",
                    StderrStartActivity {
                        act,
                        lvl,
                        typ,
                        s,
                        fields,
                        parent,
                    }
                );
            }
            Msg::StopActivity => {
                let act = read_num::<u64>(socket)
                    .await
                    .context("Failed to read act")?;
                eprintln!("[nix-daemon] stop activity: {:?}", act);
            }
            Msg::Result => {
                let res = read_string(socket).await.context("Failed to read result")?;
                eprintln!("[nix-daemon] result: {:?}", res);
            }
            Msg::Write => {
                let write = read_string(socket).await.context("Failed to read write")?;
                eprintln!("[nix-daemon] write: {:?}", write);
            }
            Msg::Last => {
                break;
            }
        }
    }
    Ok(())
}

impl DaemonConnection {
    async fn connect(&mut self) -> Result<&mut UnixStream> {
        if let Some(ref mut socket) = self.socket {
            Ok(socket)
        } else {
            let mut socket = UnixStream::connect(SOCKET_PATH)
                .await
                .with_context(|| format!("Failed to reconnect to {}", SOCKET_PATH))?;
            let data = handshake(&mut socket).await?;
            self.socket = Some(socket);
            self.server_features = data.server_features;
            self.daemon_version = data.daemon_version;
            self.is_trusted = data.is_trusted;
            Ok(self.socket.as_mut().unwrap())
        }
    }

    async fn write_num<T: Into<u64>>(&mut self, num: T) -> Result<()> {
        let socket = self.connect().await?;
        match write_num(socket, num).await {
            Err(e) => {
                self.socket = None;
                Err(e)
            }
            Ok(res) => Ok(res),
        }
    }

    pub async fn read_num<T: From<u64>>(&mut self) -> Result<T> {
        let socket = self.connect().await?;
        match read_num(socket).await {
            Err(e) => {
                self.socket = None;
                Err(e)
            }
            Ok(res) => Ok(res),
        }
    }

    async fn write_string(&mut self, s: &str) -> Result<()> {
        let socket = self.connect().await?;
        if let Err(e) = write_string(socket, s).await {
            self.socket = None;
            return Err(e);
        }
        Ok(())
    }

    async fn read_string(&mut self) -> Result<String> {
        let socket = self.connect().await?;
        match read_string(socket).await {
            Err(e) => {
                self.socket = None;
                Err(e)
            }
            Ok(res) => Ok(res),
        }
    }

    async fn read_string_list(&mut self) -> Result<Vec<String>> {
        let socket = self.connect().await?;
        match read_string_list(socket).await {
            Err(e) => {
                self.socket = None;
                Err(e)
            }
            Ok(res) => Ok(res),
        }
    }

    pub async fn forward_stderr(&mut self) -> Result<()> {
        let socket = self.connect().await?;
        if let Err(e) = forward_stderr(socket).await {
            self.socket = None;
            return Err(e);
        }
        Ok(())
    }

    async fn send_op(&mut self, op: OpCode) -> Result<()> {
        self.write_num(op as u64).await?;
        Ok(())
    }

    #[allow(dead_code)]
    async fn recv_op(&mut self) -> Result<OpCode> {
        let op = self.read_num::<u64>().await?;
        OpCode::try_from(op).context("Invalid opcode")
    }

    #[allow(dead_code)]
    pub(crate) async fn is_valid_path(&mut self, path: &str) -> Result<bool> {
        self.send_op(OpCode::IsValidPath)
            .await
            .context("Failed to send opcode")?;
        self.write_string(path)
            .await
            .context("Failed to write path")?;
        self.forward_stderr()
            .await
            .context("Failed to forward stderr")?;

        let res = self
            .read_num::<u64>()
            .await
            .context("Failed to read result")?;
        Ok(res != 0)
    }

    #[allow(dead_code)]
    pub(crate) async fn query_path_from_hash_part(
        &mut self,
        hash_part: &str,
    ) -> Result<Option<String>> {
        self.send_op(OpCode::QueryPathFromHashPart)
            .await
            .context("Failed to send opcode")?;
        self.write_string(hash_part)
            .await
            .context("Failed to write hash part")?;
        self.forward_stderr()
            .await
            .context("Failed to forward stderr")?;

        match self.read_string().await {
            Ok(resp) => {
                if resp.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(resp))
                }
            }
            Err(e) => Err(e),
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn query_path_info(&mut self, path: &str) -> Result<QueryPathInfoResponse> {
        self.send_op(OpCode::QueryPathInfo)
            .await
            .context("Failed to send opcode")?;
        self.write_string(path)
            .await
            .context("Failed to write path")?;

        self.forward_stderr()
            .await
            .context("Failed to forward stderr")?;

        let optional = self
            .read_num::<u64>()
            .await
            .context("Failed to read optional")?;
        if optional == 0 {
            return Ok(QueryPathInfoResponse { path: None });
        }
        let mut path_info = ValidPathInfo {
            deriver: self.read_string().await.context("Failed to read deriver")?,
            hash: self.read_string().await.context("Failed to read hash")?,
            references: self
                .read_string_list()
                .await
                .context("Failed to read references")?,
            registration_time: self
                .read_num()
                .await
                .context("Failed to read registration time")?,
            nar_size: self.read_num().await.context("Failed to read nar size")?,
            ultimate: self
                .read_num::<u64>()
                .await
                .context("Failed to read ultimate")?
                != 0,
            sigs: self
                .read_string_list()
                .await
                .context("Failed to read sigs")?,
            content_address: Some(
                self.read_string()
                    .await
                    .context("Failed to read content address")?,
            ),
        };
        if path_info.content_address.as_ref().unwrap().is_empty() {
            path_info.content_address = None;
        }

        Ok(QueryPathInfoResponse {
            path: Some(path_info),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    #[tokio::test]
    async fn test_nix_daemon() -> Result<()> {
        if !Path::new(SOCKET_PATH).exists() {
            return Ok(());
        }
        let mut conn: DaemonConnection = Default::default();

        // add to store
        let temp_dir = tempfile::tempdir().context("Failed to create temp dir")?;
        let temp_path = temp_dir.path().join("test.txt");
        std::fs::write(&temp_path, b"hello world").context("Failed to write to temp file")?;

        let store_path = Command::new("nix-store")
            .arg("--add")
            .arg(&temp_path)
            .output()
            .context("Failed to add to store")?;
        eprintln!("stderr: {:?}", &String::from_utf8_lossy(&store_path.stderr));
        let store_path = str::from_utf8(&store_path.stdout)
            .context("Failed to parse store path")?
            .trim()
            .to_owned();

        assert!(conn
            .is_valid_path(&store_path)
            .await
            .context("Failed to check path")
            .unwrap());

        let path_info = conn
            .query_path_info(&store_path)
            .await
            .context("Failed to check path")
            .unwrap()
            .path;
        let path_info = path_info.unwrap();
        assert_eq!(path_info.sigs.len(), 0);
        assert!(!path_info.ultimate);
        assert!(path_info.nar_size > 0, "nar size: {}", path_info.nar_size);

        let hash_part = &store_path
            .strip_prefix("/nix/store/")
            .context("cannot strip prefix")
            .unwrap()[..32];

        let res = conn
            .query_path_from_hash_part(hash_part)
            .await
            .context("Failed to get path info")
            .unwrap()
            .unwrap();
        assert_eq!(res, store_path);

        Ok(())
    }
}
