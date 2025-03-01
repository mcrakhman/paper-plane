use anyhow::Result;
use sqlx::{Row, SqlitePool};

pub struct FileDatabase {
    pool: SqlitePool,
}

pub struct FileDescription {
    pub id: String,
    pub format: String,
    pub local_path: String,
    pub timestamp: i64,
}

impl FileDatabase {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS files (
                id TEXT PRIMARY KEY NOT NULL,
                timestamp INTEGER NOT NULL,
                local_path TEXT NOT NULL,
                format TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save(&self, msg: &FileDescription) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO files (id, timestamp, local_path, format)
            VALUES (?, ?, ?, ?)"#,
        )
        .bind(&msg.id)
        .bind(&msg.timestamp)
        .bind(&msg.local_path)
        .bind(&msg.format)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<FileDescription>> {
        let row = sqlx::query(
            r#"
            SELECT id, timestamp, local_path, format
            FROM files
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| FileDescription {
            id: row.get("id"),
            timestamp: row.get("timestamp"),
            local_path: row.get("local_path"),
            format: row.get("format"),
        }))
    }

    pub async fn contains(&self, id: &str) -> Result<bool> {
        let row = sqlx::query(
            r#"
            SELECT id
            FROM files
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.is_some())
    }

    pub async fn all_file_ids(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        let mut rows = sqlx::query("SELECT id FROM files")
            .fetch_all(&self.pool)
            .await?;
        for row in rows.iter_mut() {
            ids.push(row.get("id"));
        }
        Ok(ids)
    }
}
