use serde_json::json;
use tempfile::tempdir;
use transcoderr::db;

#[tokio::test]
async fn source_crud_roundtrip() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let id = db::sources::insert(&pool, "radarr", "main", &json!({"url":"http://radarr"}), "tok").await.unwrap();
    let s = db::sources::get_by_kind_and_token(&pool, "radarr", "tok").await.unwrap().unwrap();
    assert_eq!(s.id, id);
    assert_eq!(s.name, "main");
}
