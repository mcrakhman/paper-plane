use anyhow::Result;
use chrono::{DateTime, Utc};
use ed25519_dalek::{SigningKey, VerifyingKey};
use hex;
use sqlx::{Row, SqlitePool};

pub struct PeerDatabase {
    pool: SqlitePool,
}

#[derive(Debug)]
pub struct Peer {
    pub id: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub public_key: VerifyingKey,
    pub signing_key: Option<SigningKey>,
}

impl Peer {
    pub fn new(id: String, name: String, pub_key: String) -> Result<Peer> {
        let public_key = VerifyingKey::from_bytes(
            hex::decode(pub_key)
                .map_err(|_| anyhow::anyhow!("Invalid public key hex"))?
                .as_slice().try_into()?
        )?;
        Ok(Peer {
            id,
            name: Some(name),
            created_at: Utc::now(),
            public_key,
            signing_key: None,
        })
    }
}

impl PeerDatabase {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS peers (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT,
                created_at INTEGER NOT NULL,
                public_key BLOB NOT NULL,
                signing_key BLOB
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_peer(&self, peer: &Peer) -> Result<()> {
        let public_key_bytes = peer.public_key.to_bytes();
        let signing_key_bytes = peer.signing_key.as_ref().map(|key| key.to_bytes());

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO peers (id, name, created_at, public_key, signing_key)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&peer.id)
        .bind(&peer.name)
        .bind(peer.created_at.timestamp())
        .bind(&public_key_bytes.to_vec())
        .bind(signing_key_bytes.map(|bytes| bytes.to_vec()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn create_local_peer(&self, name: Option<String>) -> Result<Peer> {
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        let peer_id = hex::encode(verifying_key.to_bytes());

        let peer = Peer {
            id: peer_id,
            name,
            created_at: Utc::now(),
            public_key: verifying_key,
            signing_key: Some(signing_key),
        };

        self.save_peer(&peer).await?;
        Ok(peer)
    }

    pub async fn get_peer_by_id(&self, id: &str) -> Result<Option<Peer>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, created_at, public_key, signing_key
            FROM peers
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let public_key_bytes: Vec<u8> = row.get("public_key");
                let public_key = VerifyingKey::from_bytes(
                    public_key_bytes[..]
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid public key bytes"))?,
                )?;

                let signing_key = match row.get::<Option<Vec<u8>>, _>("signing_key") {
                    Some(bytes) => Some(SigningKey::from_bytes(
                        bytes[..]
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("Invalid signing key bytes"))?,
                    )),
                    None => None,
                };

                Ok(Some(Peer {
                    id: row.get("id"),
                    name: row.get("name"),
                    created_at: DateTime::from_timestamp(row.get::<i64, _>("created_at"), 0)
                        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?,
                    public_key,
                    signing_key,
                }))
            }
            None => Ok(None),
        }
    }

    pub async fn get_all_peers(&self) -> Result<Vec<Peer>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, created_at, public_key, signing_key
            FROM peers
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut peers = Vec::with_capacity(rows.len());
        for row in rows {
            let public_key_bytes: Vec<u8> = row.get("public_key");
            let public_key = VerifyingKey::from_bytes(
                public_key_bytes[..]
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid public key bytes"))?,
            )?;

            let signing_key = match row.get::<Option<Vec<u8>>, _>("signing_key") {
                Some(bytes) => Some(SigningKey::from_bytes(
                    bytes[..]
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid signing key bytes"))?,
                )),
                None => None,
            };

            peers.push(Peer {
                id: row.get("id"),
                name: row.get("name"),
                created_at: DateTime::from_timestamp(row.get::<i64, _>("created_at"), 0)
                    .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?,
                public_key,
                signing_key,
            });
        }

        Ok(peers)
    }

    pub async fn get_local_peer(&self) -> Result<Option<Peer>> {
        let row = sqlx::query(
            r#"
            SELECT id, name, created_at, public_key, signing_key
            FROM peers
            WHERE signing_key IS NOT NULL
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let public_key_bytes: Vec<u8> = row.get("public_key");
                let public_key = VerifyingKey::from_bytes(
                    public_key_bytes[..]
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid public key bytes"))?,
                )?;

                let signing_key_bytes: Vec<u8> = row.get("signing_key");
                let signing_key = SigningKey::from_bytes(
                    signing_key_bytes[..]
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid signing key bytes"))?,
                );

                Ok(Some(Peer {
                    id: row.get("id"),
                    name: row.get("name"),
                    created_at: DateTime::from_timestamp(row.get::<i64, _>("created_at"), 0)
                        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?,
                    public_key,
                    signing_key: Some(signing_key),
                }))
            }
            None => Ok(None),
        }
    }
}
