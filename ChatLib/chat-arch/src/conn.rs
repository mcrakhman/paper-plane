use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use bytes::BytesMut;
use std::task::ready;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};

const NONCE_SIZE: usize = 12;
type SymKey = [u8; 32];

enum ReadState {
    ReadingLength,
    ReadingFrame { frame_len: usize },
}

enum WriteState {
    Idle,
    WritingFrame {
        buffer: BytesMut,
        offset: usize,
        data_len: usize,
    },
}

pub struct EncryptedStream<S> {
    inner: S,
    cipher: Aes256Gcm,

    read_buffer: BytesMut,
    decrypted_buffer: BytesMut,
    read_state: ReadState,

    write_state: WriteState,
}

impl<S: AsyncRead + AsyncWrite + Unpin> EncryptedStream<S> {
    pub fn new(inner: S, sym_key: &SymKey) -> Self {
        Self {
            inner,
            cipher: Aes256Gcm::new(Key::<aes_gcm::aes::Aes256>::from_slice(sym_key)),
            read_buffer: BytesMut::with_capacity(1024),
            decrypted_buffer: BytesMut::new(),
            read_state: ReadState::ReadingLength,
            write_state: WriteState::Idle,
        }
    }
}

fn read_more<S>(
    inner: &mut S,
    read_buffer: &mut BytesMut,
    cx: &mut Context<'_>,
) -> Poll<io::Result<usize>>
where
    S: AsyncRead + Unpin,
{
    let mut local_buf = [0u8; 8192];
    let mut read_buf = ReadBuf::new(&mut local_buf);

    match Pin::new(inner).poll_read(cx, &mut read_buf) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(Ok(())) => {
            let n = read_buf.filled().len();
            if n > 0 {
                read_buffer.extend_from_slice(read_buf.filled());
            }
            Poll::Ready(Ok(n))
        }
        Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for EncryptedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.decrypted_buffer.is_empty() {
                let to_read = std::cmp::min(self.decrypted_buffer.len(), buf.remaining());
                buf.put_slice(&self.decrypted_buffer.split_to(to_read));
                return Poll::Ready(Ok(()));
            }

            let this = self.as_mut().get_mut();
            match &mut this.read_state {
                ReadState::ReadingLength => {
                    if this.read_buffer.len() < 2 {
                        let n = ready!(read_more(&mut this.inner, &mut this.read_buffer, cx))?;
                        if n == 0 && this.read_buffer.is_empty() {
                            return Poll::Ready(Ok(()));
                        }
                        continue;
                    }
                    let len_bytes = this.read_buffer.split_to(2);
                    let frame_len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
                    if frame_len < NONCE_SIZE {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Frame length smaller than nonce size",
                        )));
                    }
                    this.read_state = ReadState::ReadingFrame { frame_len };
                }

                ReadState::ReadingFrame { frame_len } => {
                    if this.read_buffer.len() < *frame_len {
                        let n = ready!(read_more(&mut this.inner, &mut this.read_buffer, cx))?;
                        if n == 0 && this.read_buffer.len() < *frame_len {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "EOF before complete frame",
                            )));
                        }
                        continue;
                    }

                    let frame_data = this.read_buffer.split_to(*frame_len);
                    let (nonce_bytes, ciphertext) = frame_data.split_at(NONCE_SIZE);
                    let nonce = Nonce::from_slice(nonce_bytes);
                    let plaintext = this.cipher.decrypt(nonce, ciphertext).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Decryption failed")
                    })?;

                    this.decrypted_buffer.extend_from_slice(&plaintext);
                    this.read_state = ReadState::ReadingLength;
                }
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for EncryptedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.as_mut().get_mut();
        let cipher = &this.cipher;
        let write_state = &mut this.write_state;
        let inner = &mut this.inner;

        loop {
            match write_state {
                WriteState::Idle => {
                    let mut nonce_bytes = [0u8; NONCE_SIZE];
                    getrandom::getrandom(&mut nonce_bytes)?;
                    let nonce = Nonce::from_slice(&nonce_bytes);

                    let ciphertext = cipher
                        .encrypt(nonce, data)
                        .map_err(|_| io::Error::new(io::ErrorKind::Other, "Encryption failed"))?;

                    let frame_len = (NONCE_SIZE + ciphertext.len()) as u16;
                    let mut buffer = BytesMut::with_capacity(2 + NONCE_SIZE + ciphertext.len());
                    buffer.extend_from_slice(&frame_len.to_be_bytes());
                    buffer.extend_from_slice(&nonce_bytes);
                    buffer.extend_from_slice(&ciphertext);

                    *write_state = WriteState::WritingFrame {
                        buffer,
                        offset: 0,
                        data_len: data.len(),
                    };
                }
                WriteState::WritingFrame {
                    buffer,
                    offset,
                    data_len,
                } => {
                    let pinned_inner = Pin::new(inner);
                    let n = ready!(pinned_inner.poll_write(cx, &buffer[*offset..]))?;
                    *offset += n;

                    if *offset >= buffer.len() {
                        let data_len = *data_len;
                        *write_state = WriteState::Idle;
                        return Poll::Ready(Ok(data_len));
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();
        let write_state = &mut this.write_state; // Mutable borrow of the write_state
        let inner = &mut this.inner;

        match write_state {
            WriteState::Idle => Pin::new(inner).poll_flush(cx),
            WriteState::WritingFrame {
                buffer,
                offset,
                data_len,
            } => {
                if *offset < buffer.len() {
                    let n = ready!(Pin::new(inner).poll_write(cx, &buffer[*offset..]))?;
                    *offset += n;
                }
                if *offset >= buffer.len() {
                    self.write_state = WriteState::Idle;
                    Pin::new(&mut self.inner).poll_flush(cx)
                } else {
                    Poll::Pending
                }
            }
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
