use crate::error::{IoErrorContext, ProtocolError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A sink that writes data in framed chunks.
/// Each chunk is prefixed with its length as a u64.
/// A zero-length chunk indicates end of stream.
pub struct FramedSink<W> {
    writer: W,
    buffer: Vec<u8>,
    buffer_size: usize,
}

impl<W: AsyncWrite + Unpin> FramedSink<W> {
    pub fn new(writer: W, buffer_size: usize) -> Self {
        Self {
            writer,
            buffer: Vec::with_capacity(buffer_size),
            buffer_size,
        }
    }

    /// Write data to the sink. This may buffer data internally.
    pub async fn write(&mut self, data: &[u8]) -> Result<(), ProtocolError> {
        let mut remaining = data;

        while !remaining.is_empty() {
            let available = self.buffer_size - self.buffer.len();
            let to_copy = remaining.len().min(available);

            self.buffer.extend_from_slice(&remaining[..to_copy]);
            remaining = &remaining[to_copy..];

            if self.buffer.len() == self.buffer_size {
                self.flush_buffer().await?;
            }
        }

        Ok(())
    }

    /// Flush any buffered data as a chunk
    async fn flush_buffer(&mut self) -> Result<(), ProtocolError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Write chunk length
        let len = self.buffer.len() as u64;
        self.writer
            .write_all(&len.to_le_bytes())
            .await
            .io_context("Failed to write chunk length")?;

        // Write chunk data
        self.writer
            .write_all(&self.buffer)
            .await
            .io_context("Failed to write chunk data")?;

        self.buffer.clear();
        Ok(())
    }

    /// Finish writing, flushing any remaining data and writing the terminating zero chunk
    pub async fn finish(mut self) -> Result<W, ProtocolError> {
        // Flush any remaining buffered data
        self.flush_buffer().await?;

        // Write zero-length chunk to indicate end
        self.writer
            .write_all(&0u64.to_le_bytes())
            .await
            .io_context("Failed to write terminating chunk")?;

        Ok(self.writer)
    }
}

/// A source that reads framed chunks back into a continuous stream
pub struct FramedSource<R> {
    reader: R,
    current_chunk: Vec<u8>,
    chunk_pos: usize,
    eof: bool,
}

impl<R: AsyncRead + Unpin> FramedSource<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            current_chunk: Vec::new(),
            chunk_pos: 0,
            eof: false,
        }
    }

    /// Read the next chunk from the stream
    async fn read_next_chunk(&mut self) -> Result<(), ProtocolError> {
        // Read chunk length
        let mut len_bytes = [0u8; 8];
        self.reader
            .read_exact(&mut len_bytes)
            .await
            .io_context("Failed to read chunk length")?;

        let len = u64::from_le_bytes(len_bytes);

        if len == 0 {
            self.eof = true;
            return Ok(());
        }

        // Read chunk data
        self.current_chunk.resize(len as usize, 0);
        self.chunk_pos = 0;
        self.reader
            .read_exact(&mut self.current_chunk)
            .await
            .io_context("Failed to read chunk data")?;

        Ok(())
    }

    /// Read data from the framed source
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ProtocolError> {
        if self.eof {
            return Ok(0);
        }

        let mut total_read = 0;

        while total_read < buf.len() {
            // If we've consumed the current chunk, read the next one
            if self.chunk_pos >= self.current_chunk.len() {
                self.read_next_chunk().await?;
                if self.eof {
                    break;
                }
            }

            // Copy data from current chunk
            let remaining_in_chunk = self.current_chunk.len() - self.chunk_pos;
            let to_read = (buf.len() - total_read).min(remaining_in_chunk);

            buf[total_read..total_read + to_read]
                .copy_from_slice(&self.current_chunk[self.chunk_pos..self.chunk_pos + to_read]);

            self.chunk_pos += to_read;
            total_read += to_read;
        }

        Ok(total_read)
    }

    /// Consume the rest of the stream (used in destructors to maintain protocol state)
    pub async fn consume_to_end(&mut self) -> Result<(), ProtocolError> {
        if self.eof {
            return Ok(());
        }

        loop {
            self.read_next_chunk().await?;
            if self.eof {
                break;
            }
        }

        Ok(())
    }
}

/// Copy data from an AsyncRead source to a FramedSink
pub async fn copy_to_framed<R, W>(
    mut source: R,
    sink: &mut FramedSink<W>,
) -> Result<u64, ProtocolError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; 8192];
    let mut total = 0u64;

    loop {
        let n = source
            .read(&mut buffer)
            .await
            .io_context("Failed to read from source")?;

        if n == 0 {
            break;
        }

        sink.write(&buffer[..n]).await?;
        total += n as u64;
    }

    Ok(total)
}
