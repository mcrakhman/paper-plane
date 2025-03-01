use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::Sha256;
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DERIVATION_TEXT: &[u8] = b"p2p-chat";

pub struct Handshake {
    pub symmetric_key: [u8; 32],
    pub their_pub_key: [u8; 32],
}

impl Handshake {
    pub fn hex_key(&self) -> String {
        hex::encode(self.their_pub_key.clone())
    }
}

pub async fn read_handshake<RW: AsyncReadExt + AsyncWriteExt + Unpin>(
    transport: &mut RW,
    my_signing_key: &SigningKey,
) -> io::Result<Handshake> {
    let mut their_ephemeral_pub_bytes = [0u8; 32]; // [k]G
    transport.read_exact(&mut their_ephemeral_pub_bytes).await?;
    let their_ephemeral_pub = x25519_dalek::PublicKey::from(their_ephemeral_pub_bytes);

    let my_ephemeral_secret = x25519_dalek::StaticSecret::new(&mut OsRng);
    let my_ephemeral_pub = x25519_dalek::PublicKey::from(&my_ephemeral_secret);

    let transcript: Vec<u8> = their_ephemeral_pub
        .as_bytes()
        .iter()
        .chain(my_ephemeral_pub.as_bytes().iter())
        .copied()
        .collect();
    let my_signature = my_signing_key.sign(&transcript);

    let my_verifying_key = VerifyingKey::from(my_signing_key);

    transport.write_all(my_ephemeral_pub.as_bytes()).await?;
    transport.write_all(my_verifying_key.as_bytes()).await?;
    transport.write_all(&my_signature.to_bytes()).await?;
    transport.flush().await?;

    let mut their_verifying_key_bytes = [0u8; 32];
    let mut their_signature_bytes = [0u8; 64];
    transport.read_exact(&mut their_verifying_key_bytes).await?;
    transport.read_exact(&mut their_signature_bytes).await?;

    let their_verifying_key = VerifyingKey::from_bytes(&their_verifying_key_bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Bad verifying key"))?;
    let their_signature = Signature::from_bytes(&their_signature_bytes);

    their_verifying_key
        .verify(&transcript, &their_signature)
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "signature verify fail")
        })?;

    let shared_secret = my_ephemeral_secret.diffie_hellman(&their_ephemeral_pub);
    let shared_secret_bytes = shared_secret.to_bytes();

    let hk = Hkdf::<Sha256>::new(None, &shared_secret_bytes);
    let mut symmetric_key = [0u8; 32];
    hk.expand(DERIVATION_TEXT, &mut symmetric_key)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "HKDF expand error"))?;

    Ok(Handshake {
        symmetric_key,
        their_pub_key: their_verifying_key_bytes.clone(),
    })
}

pub async fn write_handshake<RW: AsyncReadExt + AsyncWriteExt + Unpin>(
    transport: &mut RW,
    my_signing_key: &SigningKey,
) -> io::Result<Handshake> {
    let my_ephemeral_secret = x25519_dalek::StaticSecret::new(&mut OsRng);
    let my_ephemeral_pub = x25519_dalek::PublicKey::from(&my_ephemeral_secret);

    transport.write_all(my_ephemeral_pub.as_bytes()).await?;
    transport.flush().await?;
    let mut their_ephemeral_pub_bytes = [0u8; 32];
    let mut their_verifying_key_bytes = [0u8; 32];
    let mut their_signature_bytes = [0u8; 64];

    transport.read_exact(&mut their_ephemeral_pub_bytes).await?;
    transport.read_exact(&mut their_verifying_key_bytes).await?;
    transport.read_exact(&mut their_signature_bytes).await?;

    let their_ephemeral_pub = x25519_dalek::PublicKey::from(their_ephemeral_pub_bytes);
    let their_verifying_key = VerifyingKey::from_bytes(&their_verifying_key_bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "Bad verifying key"))?;
    let their_signature = ed25519_dalek::Signature::from_bytes(&their_signature_bytes);

    let transcript: Vec<u8> = my_ephemeral_pub
        .as_bytes()
        .iter()
        .chain(their_ephemeral_pub.as_bytes().iter())
        .copied()
        .collect();

    their_verifying_key
        .verify(&transcript, &their_signature)
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "signature verify fail")
        })?;

    let my_signature = my_signing_key.sign(&transcript);
    let my_verifying_key = VerifyingKey::from(my_signing_key);

    transport.write_all(my_verifying_key.as_bytes()).await?;
    transport.write_all(&my_signature.to_bytes()).await?;
    transport.flush().await?;

    let shared_secret = my_ephemeral_secret.diffie_hellman(&their_ephemeral_pub);
    let shared_secret_bytes = shared_secret.to_bytes();

    let hk = Hkdf::<Sha256>::new(None, &shared_secret_bytes);
    let mut symmetric_key = [0u8; 32];
    hk.expand(DERIVATION_TEXT, &mut symmetric_key)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "HKDF expand error"))?;

    Ok(Handshake {
        symmetric_key,
        their_pub_key: their_verifying_key_bytes.clone(),
    })
}
