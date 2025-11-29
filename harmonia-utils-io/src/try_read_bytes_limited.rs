use std::fmt;
use std::future::poll_fn;
use std::io;
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;

use crate::wire::ZEROS;

use super::{AsyncBytesRead, TryReadU64};

#[derive(Debug, Clone)]
pub enum TryReadBytesLimited {
    ReadLen(RangeInclusive<usize>, TryReadU64),
    Fill(usize, usize),
    Done,
}

fn invalid_data<T: fmt::Display>(msg: T) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

impl TryReadBytesLimited {
    pub fn new(limit: RangeInclusive<usize>) -> Self {
        Self::ReadLen(limit, TryReadU64::new())
    }
    pub async fn read<R: AsyncBytesRead + Unpin>(
        mut self,
        reader: &mut R,
    ) -> io::Result<Option<Bytes>> {
        let mut reader = Pin::new(reader);
        poll_fn(move |cx| self.poll_reader(cx, reader.as_mut())).await
    }

    pub fn poll_reader<R>(
        &mut self,
        cx: &mut Context<'_>,
        mut reader: Pin<&mut R>,
    ) -> Poll<io::Result<Option<Bytes>>>
    where
        R: AsyncBytesRead,
    {
        loop {
            match self {
                Self::ReadLen(limit, try_read_u64) => {
                    if let Some(raw_len) = ready!(try_read_u64.poll_reader(cx, reader.as_mut()))? {
                        // Check that length is in range and convert to usize
                        let len = raw_len
                            .try_into()
                            .ok()
                            .filter(|v| limit.contains(v))
                            .ok_or_else(|| invalid_data("bytes length out of range"))?;

                        // Calculate 64bit aligned length and convert to usize
                        let aligned: usize = raw_len
                            .checked_add(7)
                            .map(|v| v & !7)
                            .ok_or_else(|| invalid_data("aligned bytes length out of range"))?
                            .try_into()
                            .map_err(invalid_data)?;

                        if aligned > 0 {
                            // Ensure that there is enough space in buffer for contents
                            reader.as_mut().prepare(aligned);
                            *self = Self::Fill(len, aligned);
                        } else {
                            *self = Self::Done;
                            return Poll::Ready(Ok(Some(Bytes::new())));
                        }
                    } else {
                        *self = Self::Done;
                        return Poll::Ready(Ok(None));
                    }
                }
                Self::Fill(len, aligned) => {
                    let mut buf = ready!(reader.as_mut().poll_fill_buf(cx))?;
                    while buf.len() < *aligned {
                        let _ = buf.split_to(0);
                        buf = ready!(reader.as_mut().poll_force_fill_buf(cx))?;
                    }
                    let mut contents = buf.split_to(*aligned);
                    reader.as_mut().consume(*aligned);

                    let padding = *aligned - *len;
                    // Ensure padding is all zeros
                    if contents[*len..] != ZEROS[..padding] {
                        *self = Self::Done;
                        return Poll::Ready(Err(invalid_data("non-zero padding")));
                    }

                    contents.truncate(*len);
                    *self = Self::Done;
                    return Poll::Ready(Ok(Some(contents)));
                }
                Self::Done => panic!("Polling completed future"),
            }
        }
    }
}
