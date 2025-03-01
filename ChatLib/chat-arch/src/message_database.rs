use std::path::Path;

use crate::models::DbMessage;
use anyhow::Result;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

pub struct MessageDatabase {
    pool: SqlitePool,
}

impl MessageDatabase {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init(&self) -> Result<Option<u64>> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY NOT NULL,
                counter INTEGER NOT NULL,
                timestamp INTEGER NOT NULL,
                order_counter INTEGER NOT NULL,
                payload BLOB NOT NULL,
                peer_id TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        let row = sqlx::query(
            r#"
            SELECT MAX(order_counter) as order_counter
            FROM messages
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get("order_counter").ok())
    }

    pub async fn save(&self, msg: &DbMessage) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO messages (id, timestamp, counter, order_counter, payload, peer_id)
            VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&msg.id)
        .bind(&msg.timestamp)
        .bind(&(msg.counter as i64))
        .bind(&(msg.order as i64))
        .bind(&msg.payload)
        .bind(&msg.peer_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn save_many<'a, I>(&self, messages: I) -> Result<()>
    where
        I: IntoIterator<Item = &'a DbMessage>,
    {
        let mut tx: Transaction<'_, Sqlite> = self.pool.begin().await?;

        for msg in messages {
            let counter = msg.counter as i64;
            let order = msg.order as i64;
            sqlx::query(
                r#"
                    INSERT INTO messages (id, timestamp, counter, order_counter, payload, peer_id)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
            )
            .bind(msg.id.clone())
            .bind(msg.timestamp)
            .bind(&counter)
            .bind(&order)
            .bind(msg.payload.clone())
            .bind(msg.peer_id.clone())
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<DbMessage>> {
        let row = sqlx::query(
            r#"
            SELECT counter, id, timestamp, payload, peer_id, order_counter
            FROM messages
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| DbMessage {
            counter: row.get("counter"),
            id: row.get("id"),
            timestamp: row.get("timestamp"),
            payload: row.get("payload"),
            order: row.get("order_counter"),
            peer_id: row.get("peer_id"),
        }))
    }

    pub async fn get_highest_counter(&self, peer_id: &str) -> Result<u64> {
        let row = sqlx::query(
            r#"
            SELECT MAX(counter) as counter
            FROM messages
            WHERE peer_id = ?
            "#,
        )
        .bind(peer_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get("counter"))
    }

    pub async fn get_after(&self, peer_id: &str, counter: u64) -> Result<Vec<DbMessage>> {
        let rows = sqlx::query(
            r#"
            SELECT counter, id, timestamp, order_counter, payload, peer_id
            FROM messages
            WHERE peer_id = ? AND counter >= ?
            ORDER BY counter
            "#,
        )
        .bind(peer_id)
        .bind(counter as i64)
        .fetch_all(&self.pool)
        .await?;

        let messages = rows
            .into_iter()
            .map(|row| DbMessage {
                counter: row.get("counter"),
                id: row.get("id"),
                timestamp: row.get("timestamp"),
                payload: row.get("payload"),
                order: row.get("order_counter"),
                peer_id: row.get("peer_id"),
            })
            .collect();

        Ok(messages)
    }

    pub async fn get_peers(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT peer_id
            FROM messages
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let peer_ids = rows.into_iter().map(|row| row.get("peer_id")).collect();

        Ok(peer_ids)
    }
}

pub async fn create_pool(db_folder: &str) -> Result<SqlitePool> {
    let path = Path::new(db_folder).join("message.db");
    let database_url = format!("sqlite:{}?mode=rwc", path.display());
    println!("database url {}", database_url);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect(&database_url)
        .await?;
    Ok(pool)
}
