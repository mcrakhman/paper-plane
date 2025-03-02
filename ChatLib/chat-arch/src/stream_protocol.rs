use anyhow::{anyhow, Result};
use log::info;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const REQUEST_FRAME: u8 = 0x01;
const RESPONSE_FRAME: u8 = 0x02;

pub trait MessageEncoding: Sized {
    fn encode_message(&self) -> Vec<u8>;

    fn decode_message(bytes: &[u8]) -> Result<Self>;
}

pub struct StreamProtocol<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    stream: Option<Stream>,
}

impl<Stream> StreamProtocol<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(stream: Stream) -> Self {
        StreamProtocol {
            stream: Some(stream),
        }
    }

    pub fn default() -> Self {
        StreamProtocol { stream: None }
    }

    fn get_stream(&mut self) -> &mut Stream {
        self.stream.as_mut().unwrap()
    }

    pub async fn send_request<M>(&mut self, message: &M) -> Result<()>
    where
        M: MessageEncoding,
    {
        let payload = message.encode_message();
        let stream = self.get_stream();
        stream.write_all(&[REQUEST_FRAME]).await?;

        let length = payload.len() as u32;
        stream.write_all(&length.to_be_bytes()).await?;
        stream.write_all(&payload).await?;
        stream.flush().await?;
        Ok(())
    }

    pub async fn read_request<M>(&mut self) -> Result<M>
    where
        M: MessageEncoding,
    {
        let mut type_buf = [0u8; 1];
        let stream = self.get_stream();
        stream.read_exact(&mut type_buf).await?;
        if type_buf[0] != REQUEST_FRAME {
            return Err(anyhow!(
                "read_request: expected 0x01 (REQUEST_FRAME), got 0x{:02X}",
                type_buf[0]
            ));
        }

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let length = u32::from_be_bytes(len_buf);

        let mut payload = vec![0u8; length as usize];
        stream.read_exact(&mut payload).await?;

        let message = M::decode_message(&payload)?;
        Ok(message)
    }

    pub async fn send_response<M>(&mut self, message: &M) -> Result<()>
    where
        M: MessageEncoding,
    {
        let payload = message.encode_message();
        let stream = self.get_stream();
        stream.write_all(&[RESPONSE_FRAME]).await?;

        let length = payload.len() as u32;
        stream.write_all(&length.to_be_bytes()).await?;

        stream.write_all(&payload).await?;
        stream.flush().await?;
        Ok(())
    }

    pub async fn send_eof(&mut self) -> Result<()> {
        let stream = self.get_stream();
        stream.write_all(&[RESPONSE_FRAME]).await?;
        let eof = 0xFFFF_FFFFu32.to_be_bytes();
        stream.write_all(&eof).await?;
        stream.flush().await?;
        Ok(())
    }

    pub async fn read_response<M>(&mut self) -> Result<Option<M>>
    where
        M: MessageEncoding,
    {
        let mut type_buf = [0u8; 1];
        let stream = self.get_stream();
        if let Err(e) = stream.read_exact(&mut type_buf).await {
            return Err(anyhow!("Failed to read response type: {}", e));
        }
        if type_buf[0] != RESPONSE_FRAME {
            return Err(anyhow!(
                "Expected RESPONSE_FRAME=0x02, got 0x{:02X}",
                type_buf[0]
            ));
        }

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let length = u32::from_be_bytes(len_buf);

        if length == 0xFFFF_FFFF {
            return Ok(None);
        }

        let mut chunk = vec![0u8; length as usize];
        stream.read_exact(&mut chunk).await?;

        let msg = M::decode_message(&chunk)?;
        Ok(Some(msg))
    }
}

impl<Stream> Drop for StreamProtocol<Stream>
where
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    fn drop(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let mut this = StreamProtocol::default();
            std::mem::swap(&mut this, self);
            tokio::spawn(async move {
                let _ = stream.flush().await;
                let _ = stream.shutdown().await;
            });
        }
    }
}
