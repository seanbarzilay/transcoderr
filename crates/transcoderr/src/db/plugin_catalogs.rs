use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct CatalogRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub auth_header: Option<String>,
    pub priority: i32,
    pub last_fetched_at: Option<i64>,
    pub last_error: Option<String>,
}

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<CatalogRow>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT id, name, url, auth_header, priority, last_fetched_at, last_error \
         FROM plugin_catalogs ORDER BY priority, name"
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| CatalogRow {
        id: r.get(0),
        name: r.get(1),
        url: r.get(2),
        auth_header: r.get(3),
        priority: r.get(4),
        last_fetched_at: r.get(5),
        last_error: r.get(6),
    }).collect())
}

pub async fn create(
    pool: &SqlitePool,
    name: &str,
    url: &str,
    auth_header: Option<&str>,
    priority: i32,
) -> sqlx::Result<i64> {
    let now = chrono::Utc::now().timestamp();
    let res = sqlx::query(
        "INSERT INTO plugin_catalogs (name, url, auth_header, priority, created_at) \
         VALUES (?, ?, ?, ?, ?)"
    )
    .bind(name).bind(url).bind(auth_header).bind(priority).bind(now)
    .execute(pool).await?;
    Ok(res.last_insert_rowid())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
    let res = sqlx::query("DELETE FROM plugin_catalogs WHERE id = ?")
        .bind(id).execute(pool).await?;
    Ok(res.rows_affected())
}

pub async fn record_fetch_success(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE plugin_catalogs SET last_fetched_at = ?, last_error = NULL WHERE id = ?"
    ).bind(now).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn record_fetch_error(pool: &SqlitePool, id: i64, err: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE plugin_catalogs SET last_error = ? WHERE id = ?"
    ).bind(err).bind(id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn open_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        // The seed row from the migration would interfere; clear it first.
        sqlx::query("DELETE FROM plugin_catalogs").execute(&pool).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn create_then_list() {
        let (pool, _dir) = open_pool().await;
        let id = create(&pool, "internal", "https://internal.example/index.json",
                        Some("Bearer xyz"), 5).await.unwrap();
        let rows = list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].name, "internal");
        assert_eq!(rows[0].auth_header.as_deref(), Some("Bearer xyz"));
        assert_eq!(rows[0].priority, 5);
        assert!(rows[0].last_fetched_at.is_none());
    }

    #[tokio::test]
    async fn list_orders_by_priority_then_name() {
        let (pool, _dir) = open_pool().await;
        create(&pool, "z-late", "https://z", None, 9).await.unwrap();
        create(&pool, "a-late", "https://a", None, 9).await.unwrap();
        create(&pool, "early",  "https://e", None, 1).await.unwrap();
        let rows = list(&pool).await.unwrap();
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["early", "a-late", "z-late"]);
    }

    #[tokio::test]
    async fn record_fetch_success_clears_last_error() {
        let (pool, _dir) = open_pool().await;
        let id = create(&pool, "x", "https://x", None, 0).await.unwrap();
        record_fetch_error(&pool, id, "boom").await.unwrap();
        record_fetch_success(&pool, id).await.unwrap();
        let rows = list(&pool).await.unwrap();
        assert!(rows[0].last_error.is_none());
        assert!(rows[0].last_fetched_at.is_some());
    }

    #[tokio::test]
    async fn delete_removes_the_row() {
        let (pool, _dir) = open_pool().await;
        let id = create(&pool, "x", "https://x", None, 0).await.unwrap();
        let removed = delete(&pool, id).await.unwrap();
        assert_eq!(removed, 1);
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
