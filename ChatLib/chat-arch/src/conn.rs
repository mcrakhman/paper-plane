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
    WritingFrame { buffer: BytesMut, offset: usize },
}

const FRAME_SIZE: usize = 8192;
const GCM_TAG_SIZE: usize = 16;
const MAX_FRAME_LEN: usize = NONCE_SIZE + FRAME_SIZE + GCM_TAG_SIZE;

pub struct EncryptedStream<S> {
    inner: S,
    cipher: Aes256Gcm,

    read_buffer: BytesMut,
    decrypted_buffer: BytesMut,
    read_state: ReadState,

    write_state: WriteState,
    read_buffer_size: usize,
}

impl<S: AsyncRead + AsyncWrite + Unpin> EncryptedStream<S> {
    pub fn new(inner: S, sym_key: &SymKey) -> Self {
        let read_buffer_size = MAX_FRAME_LEN + 2;
        Self {
            inner,
            cipher: Aes256Gcm::new(Key::<aes_gcm::aes::Aes256>::from_slice(sym_key)),
            read_buffer: BytesMut::with_capacity(read_buffer_size),
            decrypted_buffer: BytesMut::new(),
            read_state: ReadState::ReadingLength,
            write_state: WriteState::Idle,
            read_buffer_size,
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
    let n = {
        let spare = read_buffer.spare_capacity_mut();
        let mut buf = ReadBuf::uninit(spare);

        match Pin::new(inner).poll_read(cx, &mut buf) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => buf.filled().len(),
        }
    };

    unsafe {
        read_buffer.set_len(read_buffer.len() + n);
    }

    Poll::Ready(Ok(n))
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for EncryptedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
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
                        let data = this.read_buffer.split();
                        this.read_buffer.reserve(this.read_buffer_size);
                        this.read_buffer.extend_from_slice(&data);
                        let n = ready!(read_more(&mut this.inner, &mut this.read_buffer, cx))?;
                        if n == 0 {
                            if !this.read_buffer.is_empty() {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "EOF before complete frame length",
                                )));
                            }
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
                    } else if frame_len > MAX_FRAME_LEN {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Frame length larger than max frame len size",
                        )));
                    }
                    this.read_state = ReadState::ReadingFrame { frame_len };
                }

                ReadState::ReadingFrame { frame_len } => {
                    if this.read_buffer.len() < *frame_len {
                        let data = this.read_buffer.split();
                        this.read_buffer.reserve(this.read_buffer_size);
                        this.read_buffer.extend_from_slice(&data);
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

        loop {
            match &mut this.write_state {
                WriteState::WritingFrame { buffer, offset } => {
                    while *offset < buffer.len() {
                        let n = match Pin::new(&mut this.inner).poll_write(cx, &buffer[*offset..]) {
                            Poll::Pending => {
                                return Poll::Pending;
                            }

                            Poll::Ready(Err(err)) => {
                                return Poll::Ready(Err(err));
                            }

                            Poll::Ready(Ok(0)) => {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::WriteZero,
                                    "failed to write encrypted frame",
                                )));
                            }

                            Poll::Ready(Ok(n)) => n,
                        };

                        *offset += n;
                    }
                    this.write_state = WriteState::Idle;
                }

                WriteState::Idle => {
                    if data.is_empty() {
                        return Poll::Ready(Ok(0));
                    }

                    let accepted = data.len().min(FRAME_SIZE);
                    let plaintext = &data[..accepted];

                    let mut nonce_bytes = [0u8; NONCE_SIZE];
                    getrandom::getrandom(&mut nonce_bytes)?;

                    let nonce = Nonce::from_slice(&nonce_bytes);

                    let ciphertext = this
                        .cipher
                        .encrypt(nonce, plaintext)
                        .map_err(|_| io::Error::new(io::ErrorKind::Other, "encryption failed"))?;

                    let frame_len = u16::try_from(NONCE_SIZE + ciphertext.len()).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "encrypted frame too large")
                    })?;

                    let mut buffer = BytesMut::with_capacity(2 + NONCE_SIZE + ciphertext.len());

                    buffer.extend_from_slice(&frame_len.to_be_bytes());
                    buffer.extend_from_slice(&nonce_bytes);
                    buffer.extend_from_slice(&ciphertext);

                    this.write_state = WriteState::WritingFrame { buffer, offset: 0 };

                    return Poll::Ready(Ok(accepted));
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();

        loop {
            match &mut this.write_state {
                WriteState::Idle => {
                    return Pin::new(&mut this.inner).poll_flush(cx);
                }

                WriteState::WritingFrame { buffer, offset } => {
                    while *offset < buffer.len() {
                        let n = match Pin::new(&mut this.inner).poll_write(cx, &buffer[*offset..]) {
                            Poll::Pending => return Poll::Pending,

                            Poll::Ready(Err(err)) => {
                                return Poll::Ready(Err(err));
                            }

                            Poll::Ready(Ok(0)) => {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::WriteZero,
                                    "failed to write encrypted frame",
                                )));
                            }

                            Poll::Ready(Ok(n)) => n,
                        };

                        *offset += n;
                    }

                    this.write_state = WriteState::Idle;
                }
            }
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
