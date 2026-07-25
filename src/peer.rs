use crate::common::*;
use crate::database;
use hbb_common::{
    bytes::Bytes,
    log,
    rendezvous_proto::*,
    tokio::sync::{Mutex, RwLock},
    ResultType,
};
use serde_derive::{Deserialize, Serialize};
use std::{collections::HashMap, collections::HashSet, net::SocketAddr, sync::Arc, time::Instant};

type IpBlockMap = HashMap<String, ((u32, Instant), (HashSet<String>, Instant))>;
type UserStatusMap = HashMap<Vec<u8>, Arc<(Option<Vec<u8>>, bool)>>;
type IpChangesMap = HashMap<String, (Instant, HashMap<String, i32>)>;
lazy_static::lazy_static! {
    pub(crate) static ref IP_BLOCKER: Mutex<IpBlockMap> = Default::default();
    pub(crate) static ref USER_STATUS: RwLock<UserStatusMap> = Default::default();
    pub(crate) static ref IP_CHANGES: Mutex<IpChangesMap> = Default::default();
}
pub const IP_CHANGE_DUR: u64 = 180;
pub const IP_CHANGE_DUR_X2: u64 = IP_CHANGE_DUR * 2;
pub const DAY_SECONDS: u64 = 3600 * 24;
pub const IP_BLOCK_DUR: u64 = 60;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub(crate) struct PeerInfo {
    #[serde(default)]
    pub(crate) ip: String,
}

pub(crate) struct Peer {
    pub(crate) socket_addr: SocketAddr,
    pub(crate) last_reg_time: Instant,
    pub(crate) guid: Vec<u8>,
    pub(crate) uuid: Bytes,
    pub(crate) pk: Bytes,
    // pub(crate) user: Option<Vec<u8>>,
    pub(crate) info: PeerInfo,
    // pub(crate) disabled: bool,
    pub(crate) reg_pk: (u32, Instant), // how often register_pk
}

impl Default for Peer {
    fn default() -> Self {
        Self {
            socket_addr: "0.0.0.0:0".parse().unwrap(),
            last_reg_time: get_expired_time(),
            guid: Vec::new(),
            uuid: Bytes::new(),
            pk: Bytes::new(),
            info: Default::default(),
            // user: None,
            // disabled: false,
            reg_pk: (0, get_expired_time()),
        }
    }
}

pub(crate) type LockPeer = Arc<RwLock<Peer>>;

#[derive(Clone)]
pub(crate) struct PeerMap {
    map: Arc<RwLock<HashMap<String, LockPeer>>>,
    pub(crate) db: database::Database,
}

impl PeerMap {
    pub(crate) async fn new() -> ResultType<Self> {
        let db = std::env::var("DB_URL").unwrap_or({
            let mut db = "db_v2.sqlite3".to_owned();
            #[cfg(all(windows, not(debug_assertions)))]
            {
                if let Some(path) = hbb_common::config::Config::icon_path().parent() {
                    db = format!("{}\\{}", path.to_str().unwrap_or("."), db);
                }
            }
            #[cfg(not(windows))]
            {
                db = format!("./{db}");
            }
            db
        });
        log::info!("DB_URL={}", db);
        let pm = Self {
            map: Default::default(),
            db: database::Database::new(&db).await?,
        };
        Ok(pm)
    }

    #[inline]
    pub(crate) async fn update_pk(
        &mut self,
        id: String,
        peer: LockPeer,
        addr: SocketAddr,
        uuid: Bytes,
        pk: Bytes,
        ip: String,
    ) -> register_pk_response::Result {
        log::info!("update_pk {} {:?} {:?} {:?}", id, addr, uuid, pk);
        let (info_str, guid) = {
            let mut w = peer.write().await;
            w.socket_addr = addr;
            w.uuid = uuid.clone();
            w.pk = pk.clone();
            w.last_reg_time = Instant::now();
            w.info.ip = ip;
            (
                serde_json::to_string(&w.info).unwrap_or_default(),
                w.guid.clone(),
            )
        };
        if guid.is_empty() {
            match self.db.insert_peer(&id, &uuid, &pk, &info_str).await {
                Err(err) => {
                    log::error!("db.insert_peer failed: {}", err);
                    return register_pk_response::Result::SERVER_ERROR;
                }
                Ok(guid) => {
                    peer.write().await.guid = guid;
                }
            }
        } else {
            if let Err(err) = self.db.update_pk(&guid, &id, &pk, &info_str).await {
                log::error!("db.update_pk failed: {}", err);
                return register_pk_response::Result::SERVER_ERROR;
            }
            log::info!("pk updated instead of insert");
        }
        register_pk_response::Result::OK
    }

    pub(crate) async fn rename_peer(
        &self,
        old_id: &str,
        new_id: &str,
        uuid: &Bytes,
    ) -> register_pk_response::Result {
        let mut peers = self.map.write().await;
        if old_id != new_id {
            if let Some(existing) = peers.get(new_id) {
                let existing_uuid = existing.read().await.uuid.clone();
                if existing_uuid.is_empty() || existing_uuid != *uuid {
                    return register_pk_response::Result::ID_EXISTS;
                }
            }
        }

        match self.db.rename_peer(old_id, new_id, uuid).await {
            Ok(database::RenamePeerResult::Renamed) => {
                if old_id != new_id {
                    if let Some(peer) = peers.remove(old_id) {
                        peers.insert(new_id.to_owned(), peer);
                    } else if !peers.contains_key(new_id) {
                        match self.db.get_peer(new_id).await {
                            Ok(Some(peer)) => {
                                peers.insert(new_id.to_owned(), Arc::new(RwLock::new(peer.into())));
                            }
                            Ok(None) => return register_pk_response::Result::SERVER_ERROR,
                            Err(err) => {
                                log::error!("db.get_peer after rename failed: {}", err);
                                return register_pk_response::Result::SERVER_ERROR;
                            }
                        }
                    }
                }
                register_pk_response::Result::OK
            }
            Ok(database::RenamePeerResult::IdExists) => register_pk_response::Result::ID_EXISTS,
            Ok(database::RenamePeerResult::OldIdNotFound)
            | Ok(database::RenamePeerResult::UuidMismatch) => {
                register_pk_response::Result::UUID_MISMATCH
            }
            Err(err) => {
                log::error!("db.rename_peer failed: {}", err);
                register_pk_response::Result::SERVER_ERROR
            }
        }
    }

    #[inline]
    pub(crate) async fn get(&self, id: &str) -> Option<LockPeer> {
        let p = self.map.read().await.get(id).cloned();
        if p.is_some() {
            return p;
        } else if let Ok(Some(v)) = self.db.get_peer(id).await {
            let peer = Arc::new(RwLock::new(v.into()));
            self.map.write().await.insert(id.to_owned(), peer.clone());
            return Some(peer);
        }
        None
    }

    #[inline]
    pub(crate) async fn get_or(&self, id: &str) -> LockPeer {
        if let Some(p) = self.get(id).await {
            return p;
        }
        let mut w = self.map.write().await;
        if let Some(p) = w.get(id) {
            return p.clone();
        }
        let tmp = LockPeer::default();
        w.insert(id.to_owned(), tmp.clone());
        tmp
    }

    #[inline]
    pub(crate) async fn get_in_memory(&self, id: &str) -> Option<LockPeer> {
        self.map.read().await.get(id).cloned()
    }

    #[inline]
    pub(crate) async fn is_in_memory(&self, id: &str) -> bool {
        self.map.read().await.contains_key(id)
    }
}

impl From<database::Peer> for Peer {
    fn from(peer: database::Peer) -> Self {
        Self {
            guid: peer.guid,
            uuid: peer.uuid.into(),
            pk: peer.pk.into(),
            info: serde_json::from_str::<PeerInfo>(&peer.info).unwrap_or_default(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LockPeer, PeerMap};
    use crate::database::Database;
    use hbb_common::{bytes::Bytes, rendezvous_proto::register_pk_response::Result, tokio};
    use std::{path::PathBuf, sync::Arc};

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustdesk-server-peer-map-{name}-{}.sqlite3",
            uuid::Uuid::new_v4()
        ))
    }

    async fn peer_map_with_peer(path: &PathBuf, id: &str, uuid: &[u8]) -> PeerMap {
        let db = Database::new(path.to_str().unwrap()).await.unwrap();
        db.insert_peer(id, uuid, b"peer-public-key", r#"{"ip":"10.0.0.1"}"#)
            .await
            .unwrap();
        PeerMap {
            map: Default::default(),
            db,
        }
    }

    #[tokio::test]
    async fn test_rename_peer_moves_same_live_peer_to_new_id() {
        let path = test_db_path("moves-live-peer");
        let pm = peer_map_with_peer(&path, "123456789", b"device-uuid").await;
        let old_peer = pm.get("123456789").await.unwrap();

        let result = pm
            .rename_peer(
                "123456789",
                "farm-pc01",
                &Bytes::from_static(b"device-uuid"),
            )
            .await;

        assert_eq!(result, Result::OK);
        assert!(pm.get_in_memory("123456789").await.is_none());
        let new_peer = pm.get_in_memory("farm-pc01").await.unwrap();
        assert!(Arc::ptr_eq(&old_peer, &new_peer));
        assert!(pm.db.get_peer("123456789").await.unwrap().is_none());
        assert!(pm.db.get_peer("farm-pc01").await.unwrap().is_some());
        drop(pm);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_rejects_in_memory_id_conflict() {
        let path = test_db_path("in-memory-conflict");
        let pm = peer_map_with_peer(&path, "123456789", b"source-uuid").await;
        let conflicting_peer = LockPeer::default();
        conflicting_peer.write().await.uuid = Bytes::from_static(b"target-uuid");
        pm.map
            .write()
            .await
            .insert("farm-pc01".to_owned(), conflicting_peer);

        let result = pm
            .rename_peer(
                "123456789",
                "farm-pc01",
                &Bytes::from_static(b"source-uuid"),
            )
            .await;

        assert_eq!(result, Result::ID_EXISTS);
        assert!(pm.get_in_memory("123456789").await.is_none());
        assert!(pm.db.get_peer("123456789").await.unwrap().is_some());
        assert!(pm.db.get_peer("farm-pc01").await.unwrap().is_none());
        drop(pm);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_maps_uuid_mismatch() {
        let path = test_db_path("uuid-mismatch");
        let pm = peer_map_with_peer(&path, "123456789", b"owner-uuid").await;

        let result = pm
            .rename_peer(
                "123456789",
                "farm-pc01",
                &Bytes::from_static(b"different-uuid"),
            )
            .await;

        assert_eq!(result, Result::UUID_MISMATCH);
        assert!(pm.db.get_peer("123456789").await.unwrap().is_some());
        drop(pm);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_rename_peer_retry_is_idempotent() {
        let path = test_db_path("idempotent-retry");
        let pm = peer_map_with_peer(&path, "123456789", b"device-uuid").await;
        let uuid = Bytes::from_static(b"device-uuid");

        assert_eq!(
            pm.rename_peer("123456789", "farm-pc01", &uuid).await,
            Result::OK
        );
        assert_eq!(
            pm.rename_peer("123456789", "farm-pc01", &uuid).await,
            Result::OK
        );
        assert!(pm.get_in_memory("123456789").await.is_none());
        assert!(pm.get_in_memory("farm-pc01").await.is_some());
        drop(pm);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_renames_allow_only_one_owner() {
        let path = test_db_path("concurrent-renames");
        let pm = peer_map_with_peer(&path, "123456789", b"first-uuid").await;
        pm.db
            .insert_peer(
                "987654321",
                b"second-uuid",
                b"second-public-key",
                r#"{"ip":"10.0.0.2"}"#,
            )
            .await
            .unwrap();

        let first_uuid = Bytes::from_static(b"first-uuid");
        let second_uuid = Bytes::from_static(b"second-uuid");
        let (first, second) = tokio::join!(
            pm.rename_peer("123456789", "farm-pc01", &first_uuid),
            pm.rename_peer("987654321", "farm-pc01", &second_uuid)
        );

        assert!(
            (first == Result::OK && second == Result::ID_EXISTS)
                || (first == Result::ID_EXISTS && second == Result::OK)
        );
        let winner = pm.db.get_peer("farm-pc01").await.unwrap().unwrap();
        assert!(winner.uuid == b"first-uuid" || winner.uuid == b"second-uuid");
        let remaining_sources = pm.db.get_peer("123456789").await.unwrap().is_some() as u8
            + pm.db.get_peer("987654321").await.unwrap().is_some() as u8;
        assert_eq!(remaining_sources, 1);
        drop(pm);
        std::fs::remove_file(path).unwrap();
    }
}
