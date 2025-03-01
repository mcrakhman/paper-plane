use crate::models::IndexedMessage;
use anyhow::Result;
use sqlx::{Row, SqlitePool};

pub struct IndexedMessageDatabase {
    pool: SqlitePool,
}

impl IndexedMessageDatabase {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS indexed_messages (
                id TEXT PRIMARY KEY NOT NULL,
                order_id TEXT NOT NULL,
                mentions TEXT NOT NULL,
                reply TEXT,
                text TEXT NOT NULL,
                file_id TEXT,
                file_path TEXT,
                peer_id TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save(&self, msg: &IndexedMessage) -> Result<()> {
        let mentions = msg.mentions.join(",");

        sqlx::query(
            r#"
            INSERT INTO indexed_messages (id, order_id, mentions, reply, text, file_id, file_path, peer_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&msg.id)
        .bind(&msg.order_id)
        .bind(&mentions)
        .bind(&msg.reply)
        .bind(&msg.text)
        .bind(&msg.file_id)
        .bind(&msg.file_path)
        .bind(&msg.peer_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn update_file_id(
        &self,
        file_id: &str,
        file_path: &str,
    ) -> Result<Vec<IndexedMessage>> {
        let rows = sqlx::query(
            r#"
            UPDATE indexed_messages
            SET file_path = ?
            WHERE file_id = ?
            RETURNING id, order_id, mentions, reply, text, file_id, file_path, peer_id
            "#,
        )
        .bind(file_path)
        .bind(file_id)
        .fetch_all(&self.pool)
        .await?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(self.row_to_indexed_message(row)?);
        }
        Ok(messages)
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<IndexedMessage>> {
        let row = sqlx::query(
            r#"
            SELECT id, order_id, mentions, reply, text, file_id, file_path, peer_id
            FROM indexed_messages
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| self.row_to_indexed_message(row)).transpose()
    }

    pub async fn get_all_after_order_id(&self, order_id: &str) -> Result<Vec<IndexedMessage>> {
        let rows = sqlx::query(
            r#"
            SELECT id, order_id, mentions, reply, text, file_id, file_path, peer_id
            FROM indexed_messages
            WHERE order_id >= ?
            ORDER BY order_id
            "#,
        )
        .bind(order_id)
        .fetch_all(&self.pool)
        .await?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(self.row_to_indexed_message(row)?);
        }
        Ok(messages)
    }

    fn row_to_indexed_message(&self, row: sqlx::sqlite::SqliteRow) -> Result<IndexedMessage> {
        let mentions: String = row.get("mentions");
        let mentions: Vec<String> = mentions.split(',').map(|s| s.to_string()).collect();

        Ok(IndexedMessage {
            id: row.get("id"),
            order_id: row.get("order_id"),
            mentions,
            reply: row.get("reply"),
            text: row.get("text"),
            file_id: row.get("file_id"),
            file_path: row.get("file_path"),
            peer_id: row.get("peer_id"),
        })
    }
}
