use async_trait::async_trait;
use hbb_common::{log, ResultType};
use sqlx::{
    sqlite::SqliteConnectOptions, ConnectOptions, Connection, Error as SqlxError, SqliteConnection,
};
use std::{ops::DerefMut, str::FromStr};
//use sqlx::postgres::PgPoolOptions;
//use sqlx::mysql::MySqlPoolOptions;

type Pool = deadpool::managed::Pool<DbPool>;

pub struct DbPool {
    url: String,
}

#[async_trait]
impl deadpool::managed::Manager for DbPool {
    type Type = SqliteConnection;
    type Error = SqlxError;
    async fn create(&self) -> Result<SqliteConnection, SqlxError> {
        let mut opt = SqliteConnectOptions::from_str(&self.url).unwrap();
        opt.log_statements(log::LevelFilter::Debug);
        SqliteConnection::connect_with(&opt).await
    }
    async fn recycle(
        &self,
        obj: &mut SqliteConnection,
    ) -> deadpool::managed::RecycleResult<SqlxError> {
        Ok(obj.ping().await?)
    }
}

#[derive(Clone)]
pub struct Database {
    pool: Pool,
}

#[derive(Default)]
pub struct Peer {
    pub guid: Vec<u8>,
    pub id: String,
    pub uuid: Vec<u8>,
    pub pk: Vec<u8>,
    pub user: Option<Vec<u8>>,
    pub info: String,
    pub status: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenamePeerResult {
    Renamed,
    OldIdNotFound,
    UuidMismatch,
    IdExists,
}

fn is_peer_id_unique_violation(err: &SqlxError) -> bool {
    match err {
        SqlxError::Database(err) => {
            err.code().as_deref() == Some("2067")
                || err.message().contains("UNIQUE constraint failed: peer.id")
        }
        _ => false,
    }
}

impl Database {
    pub async fn new(url: &str) -> ResultType<Database> {
        if !std::path::Path::new(url).exists() {
            std::fs::File::create(url).ok();
        }
        let n: usize = std::env::var("MAX_DATABASE_CONNECTIONS")
            .unwrap_or_else(|_| "1".to_owned())
            .parse()
            .unwrap_or(1);
        log::debug!("MAX_DATABASE_CONNECTIONS={}", n);
        let pool = Pool::new(
            DbPool {
                url: url.to_owned(),
            },
            n,
        );
        let _ = pool.get().await?; // test
        let db = Database { pool };
        db.create_tables().await?;
        Ok(db)
    }

    async fn create_tables(&self) -> ResultType<()> {
        sqlx::query!(
            "
            create table if not exists peer (
                guid blob primary key not null,
                id varchar(100) not null,
                uuid blob not null,
                pk blob not null,
                created_at datetime not null default(current_timestamp),
                user blob,
                status tinyint,
                note varchar(300),
                info text not null
            ) without rowid;
            create unique index if not exists index_peer_id on peer (id);
            create index if not exists index_peer_user on peer (user);
            create index if not exists index_peer_created_at on peer (created_at);
            create index if not exists index_peer_status on peer (status);
        "
        )
        .execute(self.pool.get().await?.deref_mut())
        .await?;
        Ok(())
    }

    pub async fn get_peer(&self, id: &str) -> ResultType<Option<Peer>> {
        Ok(sqlx::query_as!(
            Peer,
            "select guid, id, uuid, pk, user, status, info from peer where id = ?",
            id
        )
        .fetch_optional(self.pool.get().await?.deref_mut())
        .await?)
    }

    pub async fn insert_peer(
        &self,
        id: &str,
        uuid: &[u8],
        pk: &[u8],
        info: &str,
    ) -> ResultType<Vec<u8>> {
        let guid = uuid::Uuid::new_v4().as_bytes().to_vec();
        sqlx::query!(
            "insert into peer(guid, id, uuid, pk, info) values(?, ?, ?, ?, ?)",
            guid,
            id,
            uuid,
            pk,
            info
        )
        .execute(self.pool.get().await?.deref_mut())
        .await?;
        Ok(guid)
    }

    pub async fn update_pk(
        &self,
        guid: &Vec<u8>,
        id: &str,
        pk: &[u8],
        info: &str,
    ) -> ResultType<()> {
        sqlx::query!(
            "update peer set id=?, pk=?, info=? where guid=?",
            id,
            pk,
            info,
            guid
        )
        .execute(self.pool.get().await?.deref_mut())
        .await?;
        Ok(())
    }

    pub(crate) async fn rename_peer(
        &self,
        old_id: &str,
        new_id: &str,
        uuid: &[u8],
    ) -> ResultType<RenamePeerResult> {
        let mut conn = self.pool.get().await?;
        let update = sqlx::query("update peer set id=? where id=? and uuid=?")
            .bind(new_id)
            .bind(old_id)
            .bind(uuid)
            .execute(conn.deref_mut())
            .await;

        match update {
            Ok(result) if result.rows_affected() == 1 => Ok(RenamePeerResult::Renamed),
            Err(err) if is_peer_id_unique_violation(&err) => Ok(RenamePeerResult::IdExists),
            Err(err) => Err(err.into()),
            Ok(_) => {
                let old_uuid: Option<Vec<u8>> =
                    sqlx::query_scalar("select uuid from peer where id=?")
                        .bind(old_id)
                        .fetch_optional(conn.deref_mut())
                        .await?;
                if old_uuid.is_some() {
                    return Ok(RenamePeerResult::UuidMismatch);
                }

                let new_uuid: Option<Vec<u8>> =
                    sqlx::query_scalar("select uuid from peer where id=?")
                        .bind(new_id)
                        .fetch_optional(conn.deref_mut())
                        .await?;
                match new_uuid {
                    Some(stored_uuid) if stored_uuid == uuid => Ok(RenamePeerResult::Renamed),
                    Some(_) => Ok(RenamePeerResult::IdExists),
                    None => Ok(RenamePeerResult::OldIdNotFound),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, RenamePeerResult};
    use hbb_common::tokio;
    use sqlx::Row;
    use std::{ops::DerefMut, path::PathBuf};

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustdesk-server-{name}-{}.sqlite3",
            uuid::Uuid::new_v4()
        ))
    }

    async fn insert_test_peer(db: &Database, id: &str, uuid: &[u8]) {
        db.insert_peer(id, uuid, b"peer-public-key", r#"{"ip":"10.0.0.1"}"#)
            .await
            .unwrap();
    }

    #[test]
    fn test_insert() {
        insert();
    }

    #[tokio::main(flavor = "multi_thread")]
    async fn insert() {
        let db = super::Database::new("test.sqlite3").await.unwrap();
        let mut jobs = vec![];
        for i in 0..10000 {
            let cloned = db.clone();
            let id = i.to_string();
            let a = tokio::spawn(async move {
                let empty_vec = Vec::new();
                cloned
                    .insert_peer(&id, &empty_vec, &empty_vec, "")
                    .await
                    .unwrap();
            });
            jobs.push(a);
        }
        for i in 0..10000 {
            let cloned = db.clone();
            let id = i.to_string();
            let a = tokio::spawn(async move {
                cloned.get_peer(&id).await.unwrap();
            });
            jobs.push(a);
        }
        hbb_common::futures::future::join_all(jobs).await;
    }

    #[tokio::test]
    async fn test_rename_peer_preserves_peer_fields() {
        let path = test_db_path("rename-preserves-fields");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        let old_id = "123456789";
        let new_id = "farm-pc01";
        let uuid = b"device-uuid";
        insert_test_peer(&db, old_id, uuid).await;

        let mut conn = db.pool.get().await.unwrap();
        sqlx::query("update peer set user=?, status=?, note=? where id=?")
            .bind(b"workshop".as_slice())
            .bind(0_i64)
            .bind("primary farm computer")
            .bind(old_id)
            .execute(conn.deref_mut())
            .await
            .unwrap();
        let created_at: String = sqlx::query_scalar("select created_at from peer where id=?")
            .bind(old_id)
            .fetch_one(conn.deref_mut())
            .await
            .unwrap();
        drop(conn);

        let result = db.rename_peer(old_id, new_id, uuid).await.unwrap();

        assert_eq!(result, RenamePeerResult::Renamed);
        assert!(db.get_peer(old_id).await.unwrap().is_none());
        let peer = db.get_peer(new_id).await.unwrap().unwrap();
        assert_eq!(peer.uuid, uuid);
        assert_eq!(peer.pk, b"peer-public-key");
        assert_eq!(peer.user, Some(b"workshop".to_vec()));
        assert_eq!(peer.status, Some(0));
        assert_eq!(peer.info, r#"{"ip":"10.0.0.1"}"#);

        let mut conn = db.pool.get().await.unwrap();
        let row = sqlx::query("select created_at, note from peer where id=?")
            .bind(new_id)
            .fetch_one(conn.deref_mut())
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("created_at"), created_at);
        assert_eq!(row.get::<String, _>("note"), "primary farm computer");
        drop(conn);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_rejects_uuid_mismatch() {
        let path = test_db_path("rename-uuid-mismatch");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        insert_test_peer(&db, "123456789", b"owner-uuid").await;

        let result = db
            .rename_peer("123456789", "farm-pc01", b"different-uuid")
            .await
            .unwrap();

        assert_eq!(result, RenamePeerResult::UuidMismatch);
        assert!(db.get_peer("123456789").await.unwrap().is_some());
        assert!(db.get_peer("farm-pc01").await.unwrap().is_none());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_rejects_duplicate_id() {
        let path = test_db_path("rename-duplicate");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        insert_test_peer(&db, "123456789", b"source-uuid").await;
        insert_test_peer(&db, "farm-pc01", b"target-uuid").await;

        let result = db
            .rename_peer("123456789", "farm-pc01", b"source-uuid")
            .await
            .unwrap();

        assert_eq!(result, RenamePeerResult::IdExists);
        assert!(db.get_peer("123456789").await.unwrap().is_some());
        let target = db.get_peer("farm-pc01").await.unwrap().unwrap();
        assert_eq!(target.uuid, b"target-uuid");
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_is_idempotent_after_success() {
        let path = test_db_path("rename-idempotent");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        let uuid = b"device-uuid";
        insert_test_peer(&db, "123456789", uuid).await;

        assert_eq!(
            db.rename_peer("123456789", "farm-pc01", uuid)
                .await
                .unwrap(),
            RenamePeerResult::Renamed
        );
        assert_eq!(
            db.rename_peer("123456789", "farm-pc01", uuid)
                .await
                .unwrap(),
            RenamePeerResult::Renamed
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_reports_missing_old_id() {
        let path = test_db_path("rename-missing-old-id");
        let db = Database::new(path.to_str().unwrap()).await.unwrap();

        let result = db
            .rename_peer("123456789", "farm-pc01", b"device-uuid")
            .await
            .unwrap();

        assert_eq!(result, RenamePeerResult::OldIdNotFound);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
