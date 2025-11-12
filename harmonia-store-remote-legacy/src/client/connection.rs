use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{
    CURRENT_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, WORKER_MAGIC_1, WORKER_MAGIC_2,
};
use crate::protocol::{
    LoggerField, Msg, OpCode, ProtocolVersion, StderrError, StderrStartActivity, Trace,
};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::store_path::StoreDir;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

#[derive(Debug)]
pub struct Connection {
    stream: UnixStream,
    store_dir: StoreDir,
}

impl Connection {
    pub async fn connect(
        path: &Path,
        store_dir: StoreDir,
    ) -> Result<(Self, ProtocolVersion, Vec<Vec<u8>>), ProtocolError> {
        let mut stream = UnixStream::connect(path)
            .await
            .io_context(format!("Failed to connect to {path:?}"))?;

        // Handshake - use store_dir for serialization
        WORKER_MAGIC_1
            .serialize(&mut stream, CURRENT_PROTOCOL_VERSION, &store_dir)
            .await?;

        let magic = u64::deserialize(&mut stream, CURRENT_PROTOCOL_VERSION, &store_dir).await?;
        if magic != WORKER_MAGIC_2 {
            return Err(ProtocolError::InvalidMagic {
                expected: WORKER_MAGIC_2,
                actual: magic,
            });
        }

        let server_version = ProtocolVersion::from(
            u64::deserialize(&mut stream, CURRENT_PROTOCOL_VERSION, &store_dir).await?,
        );

        if server_version < MIN_PROTOCOL_VERSION {
            return Err(ProtocolError::IncompatibleVersion {
                server: server_version,
                min: MIN_PROTOCOL_VERSION,
                max: CURRENT_PROTOCOL_VERSION,
            });
        }

        // Send client version
        u64::from(CURRENT_PROTOCOL_VERSION)
            .serialize(&mut stream, CURRENT_PROTOCOL_VERSION, &store_dir)
            .await?;

        // Obsolete fields
        0u64.serialize(&mut stream, CURRENT_PROTOCOL_VERSION, &store_dir)
            .await?; // cpu affinity
        0u64.serialize(&mut stream, CURRENT_PROTOCOL_VERSION, &store_dir)
            .await?; // reserve space

        // Exchange features (if protocol >= 1.38)
        let features = if server_version
            >= (ProtocolVersion {
                major: 1,
                minor: 38,
            }) {
            let server_features =
                Vec::<Vec<u8>>::deserialize(&mut stream, server_version, &store_dir).await?;
            Vec::<Vec<u8>>::new()
                .serialize(&mut stream, server_version, &store_dir)
                .await?;
            server_features
        } else {
            Vec::new()
        };

        // Read daemon version string
        let _daemon_version =
            <Vec<u8>>::deserialize(&mut stream, server_version, &store_dir).await?;

        // Read trust status
        let _is_trusted = bool::deserialize(&mut stream, server_version, &store_dir).await?;

        let mut conn = Connection { stream, store_dir };
        conn.process_stderr().await?;

        Ok((conn, server_version, features))
    }

    pub async fn send_opcode(&mut self, opcode: OpCode) -> Result<(), ProtocolError> {
        (opcode as u64)
            .serialize(&mut self.stream, CURRENT_PROTOCOL_VERSION, &self.store_dir)
            .await
    }

    pub async fn process_stderr(&mut self) -> Result<(), ProtocolError> {
        loop {
            let msg_code =
                u64::deserialize(&mut self.stream, CURRENT_PROTOCOL_VERSION, &self.store_dir)
                    .await?;
            let msg = Msg::try_from(msg_code)?;

            match msg {
                Msg::Error => {
                    let mut err = StderrError {
                        typ: String::deserialize(
                            &mut self.stream,
                            CURRENT_PROTOCOL_VERSION,
                            &self.store_dir,
                        )
                        .await?,
                        level: u64::deserialize(
                            &mut self.stream,
                            CURRENT_PROTOCOL_VERSION,
                            &self.store_dir,
                        )
                        .await?,
                        name: String::deserialize(
                            &mut self.stream,
                            CURRENT_PROTOCOL_VERSION,
                            &self.store_dir,
                        )
                        .await?,
                        message: String::deserialize(
                            &mut self.stream,
                            CURRENT_PROTOCOL_VERSION,
                            &self.store_dir,
                        )
                        .await?,
                        have_pos: u64::deserialize(
                            &mut self.stream,
                            CURRENT_PROTOCOL_VERSION,
                            &self.store_dir,
                        )
                        .await?,
                        traces: Vec::new(),
                    };

                    let traces_len = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    for _ in 0..traces_len {
                        err.traces.push(Trace {
                            have_pos: u64::deserialize(
                                &mut self.stream,
                                CURRENT_PROTOCOL_VERSION,
                                &self.store_dir,
                            )
                            .await?,
                            trace: String::deserialize(
                                &mut self.stream,
                                CURRENT_PROTOCOL_VERSION,
                                &self.store_dir,
                            )
                            .await?,
                        });
                    }

                    return Err(ProtocolError::DaemonError {
                        message: err.message,
                    });
                }
                Msg::Next => {
                    let next = String::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    eprintln!("[nix-daemon]: {next}");
                }
                Msg::StartActivity => {
                    let act = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    let lvl = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    let typ = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    let s = String::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    let field_type = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    let fields = match field_type {
                        0 => LoggerField::Int(
                            u64::deserialize(
                                &mut self.stream,
                                CURRENT_PROTOCOL_VERSION,
                                &self.store_dir,
                            )
                            .await?,
                        ),
                        1 => LoggerField::String(
                            String::deserialize(
                                &mut self.stream,
                                CURRENT_PROTOCOL_VERSION,
                                &self.store_dir,
                            )
                            .await?,
                        ),
                        _ => return Err(ProtocolError::InvalidMsgCode(field_type)),
                    };
                    let parent = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;

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
                    let act = u64::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    eprintln!("[nix-daemon] stop activity: {act:?}");
                }
                Msg::Result => {
                    let res = String::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    eprintln!("[nix-daemon] result: {res:?}");
                }
                Msg::Write => {
                    let write = String::deserialize(
                        &mut self.stream,
                        CURRENT_PROTOCOL_VERSION,
                        &self.store_dir,
                    )
                    .await?;
                    eprintln!("[nix-daemon] write: {write:?}");
                }
                Msg::Last => {
                    break;
                }
            }
        }
        Ok(())
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
