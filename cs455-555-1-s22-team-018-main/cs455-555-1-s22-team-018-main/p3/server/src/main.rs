use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

type TermID = u32;

/// Secret used to authenticate replication requests
const REPLICATION_KEY: &str = "d6Z6ESj7/DPZKNICF0fTpQJL8TYx0+9ZT4mreDSpZth+451hiWHF05VCXFeRHIqRjU6gM+7MaX2eM8L3+g4tE/BCKr6v3XdHHA66jnVBor2gc7uMlgvaqaTtXaBqSuZImNnaZ/9w55zML+RnskNpXAF63WahE4MZsYaimmn0bDI=";

#[derive(Deserialize, Serialize)]
pub enum AppendEntriesResult {
    Success,
    Failure { term: TermID },
}

#[derive(Deserialize, Serialize)]
pub struct RequestVoteResult {
    term: TermID,
    vote_granted: bool,
}

#[jsonrpc_derive::rpc]
pub trait S2SRpc {
    #[rpc(name = "append_entries")]
    fn append_entries(
        &self,
        key: String,
        term: TermID,
        leader_id: std::net::SocketAddr,
        prev_log_index: u64,
        prev_log_term: TermID,
        entries: Vec<ReplicationLogEntry>,
    ) -> Result<AppendEntriesResult, jsonrpc_core::Error>;

    #[rpc(name = "request_vote")]
    fn request_vote(
        &self,
        key: String,
        term: TermID,
        candidate_id: std::net::SocketAddr,
        last_log_index: u64,
        last_log_term: TermID,
    ) -> Result<RequestVoteResult, jsonrpc_core::Error>;

    #[rpc(name = "install_snapshot")]
    fn install_snapshot(
        &self,
        key: String,
        term: TermID,
        leader_id: std::net::SocketAddr,
        last_included_index: u64,
        last_included_term: TermID,
        data: String,
    ) -> Result<TermID, jsonrpc_core::Error>;
}

struct CurrentTermState {
    id: TermID,
    voted_for: Option<std::net::SocketAddr>,
}

impl CurrentTermState {
    pub fn new(id: TermID) -> Self {
        Self {
            id,
            voted_for: None,
        }
    }
}

struct LeaderFollowerState {
    next_index: u64,
}

impl LeaderFollowerState {
    fn new(next_index: u64) -> Self {
        Self { next_index }
    }
}

enum ReplicationRole {
    Follower,
    Candidate,
    Leader,
}

enum ReplicationRoleState {
    Follower,
    Candidate {
        received_votes: HashSet<std::net::SocketAddr>,
    },
    Leader {
        follower_states: HashMap<std::net::SocketAddr, LeaderFollowerState>,
    },
}

impl ReplicationRoleState {
    fn role(&self) -> ReplicationRole {
        match self {
            ReplicationRoleState::Follower => ReplicationRole::Follower,
            ReplicationRoleState::Candidate { .. } => ReplicationRole::Candidate,
            ReplicationRoleState::Leader { .. } => ReplicationRole::Leader,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReplicationLogEntry {
    term: TermID,
    operation: DatabaseOperation,
}

struct ReplicationState {
    my_id: std::net::SocketAddr,
    current_term: CurrentTermState,
    log_start: u64,
    log: Vec<ReplicationLogEntry>,
    role: ReplicationRoleState,
    pub leader_id: Option<std::net::SocketAddr>,
    node_list: Arc<HashMap<std::net::SocketAddr, S2SRpcClient>>,
    last_snapshot: Option<(Snapshot, String)>,
}

impl ReplicationState {
    fn new(
        my_id: std::net::SocketAddr,
        node_list: HashMap<std::net::SocketAddr, S2SRpcClient>,
    ) -> Self {
        Self {
            my_id,
            current_term: CurrentTermState::new(0),
            log_start: 1,
            log: Vec::new(),
            role: ReplicationRoleState::Follower,
            leader_id: None,
            node_list: Arc::new(node_list),
            last_snapshot: None,
        }
    }

    fn add_entry(&mut self, operation: DatabaseOperation, db: &mut Database) {
        db.apply(operation.clone());

        self.log.push(ReplicationLogEntry {
            term: self.current_term.id,
            operation,
        });
    }
}

const ELECTION_TIMEOUT_RANGE: std::ops::Range<std::time::Duration> =
    std::time::Duration::from_millis(150)..std::time::Duration::from_millis(300);

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

async fn run_replication_task(
    state_mutex: Arc<RwLock<ReplicationState>>,
    mut replication_ping_recv_recv: tokio::sync::mpsc::Receiver<()>,
    replication_ping_recv_send: tokio::sync::mpsc::Sender<()>,
) {
    let become_candidate = |expected_term| {
        let replication_ping_recv_send = replication_ping_recv_send.clone(); // don't know why this is necessary

        let mut lock = state_mutex.write().unwrap();
        let state = &mut *lock;

        if state.current_term.id != expected_term {
            // term has changed, try loop again
            return;
        }

        let new_term = state.current_term.id + 1;

        println!("Becoming candidate for term {}", new_term);

        state.current_term = CurrentTermState::new(new_term);

        let mut received_votes = HashSet::new();

        // vote for self
        received_votes.insert(state.my_id);
        state.current_term.voted_for = Some(state.my_id);

        state.role = ReplicationRoleState::Candidate { received_votes };

        let my_id = state.my_id;
        let node_list = state.node_list.clone();
        let last_log_index = state.log_start + state.log.len() as u64;
        let last_log_term = state.log.last().map(|entry| entry.term).unwrap_or(0);
        let _ = lock;

        let state_mutex = state_mutex.clone();

        tokio::spawn(async move {
            for (id, node) in node_list.iter() {
                if *id == my_id {
                    continue;
                }

                let res = node
                    .request_vote(
                        REPLICATION_KEY.to_owned(),
                        new_term,
                        my_id,
                        last_log_index,
                        last_log_term,
                    )
                    .await;
                let res = match res {
                    Ok(res) => res,
                    Err(err) => {
                        eprintln!("Failed to reach peer for vote: {:?}", err);
                        continue;
                    }
                };

                let mut lock = state_mutex.write().unwrap();
                let state = &mut *lock;

                if state.current_term.id != new_term {
                    break;
                }

                if let ReplicationRoleState::Candidate {
                    ref mut received_votes,
                } = state.role
                {
                    if res.vote_granted {
                        received_votes.insert(*id);
                        if received_votes.len() > node_list.len() / 2 {
                            // received majority, become leader

                            println!("Becoming leader");
                            state.role = ReplicationRoleState::Leader {
                                follower_states: node_list
                                    .iter()
                                    .map(|(id, _)| {
                                        (
                                            *id,
                                            LeaderFollowerState::new(
                                                (state.log.len() as u64) + state.log_start,
                                            ),
                                        )
                                    })
                                    .collect(),
                            };
                            state.leader_id = Some(my_id);

                            break;
                        }
                    }
                } else {
                    // no longer candidate, stop soliciting votes
                    break;
                }
            }

            let _ = replication_ping_recv_send.try_send(());
        });
    };

    'outer: loop {
        let (role, term_id, node_list, my_id) = {
            let lock = state_mutex.read().unwrap();
            (
                lock.role.role(),
                lock.current_term.id,
                lock.node_list.clone(),
                lock.my_id,
            )
        };
        match role {
            ReplicationRole::Follower => {
                let interval = rand::thread_rng().gen_range(ELECTION_TIMEOUT_RANGE);
                match futures_util::future::select(
                    Box::pin(tokio::time::sleep(interval)),
                    Box::pin(replication_ping_recv_recv.recv()),
                )
                .await
                {
                    futures_util::future::Either::Left(((), _)) => {
                        println!("election timeout elapsed, becoming candidate");
                        become_candidate(term_id);
                    }
                    futures_util::future::Either::Right((Some(()), _)) => {
                        // got heartbeat, everything's fine
                        println!("got heartbeat");
                    }
                    futures_util::future::Either::Right((None, _)) => {
                        panic!("lost ping channel");
                    }
                }
            }
            ReplicationRole::Candidate => {
                let interval = rand::thread_rng().gen_range(ELECTION_TIMEOUT_RANGE);

                match futures_util::future::select(
                    Box::pin(tokio::time::sleep(interval)),
                    Box::pin(replication_ping_recv_recv.recv()),
                )
                .await
                {
                    futures_util::future::Either::Left(((), _)) => {
                        println!("election timeout elapsed, restarting election");
                        become_candidate(term_id);
                    }
                    futures_util::future::Either::Right((Some(()), _)) => {
                        // something happened, restart loop
                    }
                    futures_util::future::Either::Right((None, _)) => {
                        panic!("lost ping channel");
                    }
                }
            }
            ReplicationRole::Leader => {
                for (id, node) in node_list.iter() {
                    if *id == my_id {
                        continue;
                    }

                    enum Action {
                        SendEntries((u64, TermID, Vec<ReplicationLogEntry>)),
                        SendSnapshot((u64, TermID, String)),
                    }

                    let res = {
                        let lock = state_mutex.read().unwrap();

                        if let ReplicationRoleState::Leader { follower_states } = &lock.role {
                            let prev_log_index = follower_states[id].next_index - 1;

                            if prev_log_index < lock.log_start - 1 {
                                // too far behind, send snapshot

                                if let Some((snapshot, snapshot_str)) = &lock.last_snapshot {
                                    Action::SendSnapshot((
                                        snapshot.last_included_index,
                                        snapshot.last_included_term,
                                        snapshot_str.clone(),
                                    ))
                                } else {
                                    panic!("missing snapshot for older items");
                                }
                            } else {
                                let prev_log_term = if prev_log_index == 0 {
                                    0
                                } else if prev_log_index == lock.log_start - 1 {
                                    lock.last_snapshot.as_ref().unwrap().0.last_included_term
                                } else {
                                    lock.log[(prev_log_index - lock.log_start) as usize].term
                                };

                                Action::SendEntries((
                                    prev_log_index,
                                    prev_log_term,
                                    lock.log[((prev_log_index + 1 - lock.log_start) as usize)..]
                                        .to_vec(),
                                ))
                            }
                        } else {
                            continue 'outer;
                        }
                    };

                    match res {
                        Action::SendEntries((prev_log_index, prev_log_term, entries)) => {
                            let entries_len = entries.len();

                            println!("sending {} items to {} (term {})", entries_len, id, term_id);

                            let res = node
                                .append_entries(
                                    REPLICATION_KEY.to_owned(),
                                    term_id,
                                    my_id,
                                    prev_log_index,
                                    prev_log_term,
                                    entries,
                                )
                                .await;
                            match res {
                                Err(err) => {
                                    eprintln!("Failed to replicate log to peer: {:?}", err);
                                }
                                Ok(AppendEntriesResult::Success) => {
                                    let mut lock = state_mutex.write().unwrap();

                                    if let ReplicationRoleState::Leader { follower_states } =
                                        &mut lock.role
                                    {
                                        follower_states.get_mut(id).unwrap().next_index =
                                            prev_log_index + (entries_len as u64) + 1;
                                    } else {
                                        continue 'outer;
                                    }
                                }
                                Ok(AppendEntriesResult::Failure {
                                    term: follower_term,
                                }) => {
                                    let mut lock = state_mutex.write().unwrap();
                                    if follower_term > term_id {
                                        if follower_term > lock.current_term.id {
                                            lock.current_term =
                                                CurrentTermState::new(follower_term);

                                            lock.role = ReplicationRoleState::Follower;
                                        }
                                    } else {
                                        if let ReplicationRoleState::Leader { follower_states } =
                                            &mut lock.role
                                        {
                                            // assume failure is due to outdated log, decrement next_index
                                            follower_states.get_mut(id).unwrap().next_index -= 1;
                                        } else {
                                            continue 'outer;
                                        }
                                    }
                                }
                            }
                        }
                        Action::SendSnapshot((
                            last_included_index,
                            last_included_term,
                            snapshot_str,
                        )) => {
                            println!("sending snapshot to {}", id);
                            match node
                                .install_snapshot(
                                    REPLICATION_KEY.to_owned(),
                                    term_id,
                                    my_id,
                                    last_included_index,
                                    last_included_term,
                                    snapshot_str,
                                )
                                .await
                            {
                                Err(err) => {
                                    eprintln!("Failed to send snapshot to peer: {:?}", err);
                                }
                                Ok(follower_term) => {
                                    let mut lock = state_mutex.write().unwrap();
                                    if follower_term > term_id {
                                        if follower_term > lock.current_term.id {
                                            lock.current_term =
                                                CurrentTermState::new(follower_term);

                                            lock.role = ReplicationRoleState::Follower;
                                        }
                                    } else {
                                        if let ReplicationRoleState::Leader { follower_states } =
                                            &mut lock.role
                                        {
                                            follower_states.get_mut(id).unwrap().next_index =
                                                last_included_index + 1;
                                        } else {
                                            continue 'outer;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum DatabaseOperation {
    Insert(uuid::Uuid, FullUserInfo),
    Delete(uuid::Uuid),
    ChangeLoginname(uuid::Uuid, String),
}

///Structure used by server to store user information
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct FullUserInfo {
    loginname: String,
    real_name: Option<String>,
    password_hash: Option<String>,
}

///Error used if user tries to submit existing login name
#[derive(Debug, PartialEq)]
enum InsertError {
    LoginnameConflict,
}

impl InsertError {
    pub fn into_rpc_error(self) -> jsonrpc_core::Error {
        match self {
            InsertError::LoginnameConflict => {
                jsonrpc_core::Error::invalid_params("Specified loginname is already in use")
            }
        }
    }
}

///Error used if server fails to load from save file.
#[derive(Debug)]
enum LoadError {
    Integrity,
    Deserialize(serde_json::Error),
}

impl From<serde_json::Error> for LoadError {
    fn from(src: serde_json::Error) -> Self {
        Self::Deserialize(src)
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    last_included_index: u64,
    last_included_term: TermID,
    id_map: HashMap<uuid::Uuid, FullUserInfo>,
}

///Structure for database where user information is stored
struct Database {
    id_map: HashMap<uuid::Uuid, FullUserInfo>,
    name_map: HashMap<String, uuid::Uuid>,
}

///Functions that can be run on the database through the server
impl Database {
    pub fn new() -> Self {
        Self {
            id_map: Default::default(),
            name_map: Default::default(),
        }
    }

    pub fn load_from_snapshot(snapshot: Snapshot) -> Result<Self, LoadError> {
        let mut name_map: HashMap<String, uuid::Uuid> = Default::default();

        for (id, info) in snapshot.id_map.iter() {
            if name_map.insert(info.loginname.clone(), *id).is_some() {
                return Err(LoadError::Integrity);
            }
        }

        Ok(Self {
            id_map: snapshot.id_map,
            name_map,
        })
    }

    ///Returns user information from id
    pub fn get_by_id(&self, id: uuid::Uuid) -> Option<&FullUserInfo> {
        self.id_map.get(&id)
    }

    ///Returns user information from login name
    pub fn get_by_loginname(&self, loginname: &str) -> Option<(uuid::Uuid, &FullUserInfo)> {
        match self.name_map.get(loginname) {
            None => None,
            Some(id) => Some((*id, self.id_map.get(id).unwrap())),
        }
    }

    pub fn contains_loginname(&self, loginname: &str) -> bool {
        self.name_map.contains_key(loginname)
    }

    pub fn contains_id(&self, id: uuid::Uuid) -> bool {
        self.id_map.contains_key(&id)
    }

    ///Returns iterator to list all items in database
    pub fn iter(&self) -> impl Iterator<Item = (&uuid::Uuid, &FullUserInfo)> {
        self.id_map.iter()
    }

    pub fn apply(&mut self, operation: DatabaseOperation) {
        match operation {
            DatabaseOperation::Insert(id, info) => {
                self.name_map.insert(info.loginname.clone(), id);
                self.id_map.insert(id, info);
            }
            DatabaseOperation::Delete(id) => match self.id_map.get(&id) {
                None => {}
                Some(info) => {
                    self.name_map.remove(&info.loginname);
                    self.id_map.remove(&id);
                }
            },
            DatabaseOperation::ChangeLoginname(id, new_name) => match self.id_map.get_mut(&id) {
                None => {}
                Some(info) => {
                    self.name_map.remove(&info.loginname);
                    self.name_map.insert(new_name.clone(), id);
                    info.loginname = new_name;
                }
            },
        }
    }
}

fn to_internal_error<E: std::fmt::Debug>(err: E) -> jsonrpc_core::Error {
    eprintln!("{:?}", err);
    jsonrpc_core::Error::internal_error()
}

fn check_password(given: Option<&str>, hash: &Option<String>) -> Result<bool, bcrypt::BcryptError> {
    match hash {
        Some(hash) => match given {
            Some(given) => bcrypt::verify(given, hash),
            None => Ok(false),
        },
        None => Ok(true),
    }
}

fn leader_or_err(state: &ReplicationState) -> Result<(), jsonrpc_core::Error> {
    if let ReplicationRoleState::Leader { .. } = state.role {
        Ok(())
    } else {
        let mut err = jsonrpc_core::Error::new(jsonrpc_core::ErrorCode::ServerError(
            id_common::NOT_LEADER_ERROR,
        ));
        err.data = Some(
            serde_json::to_value(&id_common::NotLeaderErrorData {
                leader_id: state.leader_id,
            })
            .unwrap(),
        );

        Err(err)
    }
}

///Implementation of RPC service
#[derive(Clone)]
struct Service {
    db: Arc<RwLock<Database>>,
    replication_state: Arc<RwLock<ReplicationState>>,
    replication_ping_recv_send: tokio::sync::mpsc::Sender<()>,
}

///Handle client commands
impl id_common::Rpc for Service {
    fn lookup(
        &self,
        loginname: String,
    ) -> Result<Option<id_common::UserInfo>, jsonrpc_core::Error> {
        let lock = self.db.read().unwrap();
        let db = &*lock;

        Ok(db
            .get_by_loginname(&loginname)
            .map(|(id, info)| id_common::UserInfo {
                loginname: info.loginname.clone(),
                uuid: id,
                real_name: info.real_name.clone(),
            }))
    }
    fn reverse_lookup(
        &self,
        uuid: uuid::Uuid,
    ) -> Result<Option<id_common::UserInfo>, jsonrpc_core::Error> {
        let lock = self.db.read().unwrap();
        let db = &*lock;

        Ok(db.get_by_id(uuid).map(|info| id_common::UserInfo {
            loginname: info.loginname.clone(),
            uuid: uuid,
            real_name: info.real_name.clone(),
        }))
    }

    fn create(
        &self,
        loginname: String,
        real_name: Option<String>,
        password: Option<String>,
    ) -> Result<uuid::Uuid, jsonrpc_core::Error> {
        let mut db_lock = self.db.write().unwrap();
        let db = &mut *db_lock;

        leader_or_err(&self.replication_state.read().unwrap())?;

        let password_hash = password
            .map(|password| bcrypt::hash(password, bcrypt::DEFAULT_COST))
            .transpose()
            .map_err(to_internal_error)?;

        if db.contains_loginname(&loginname) {
            Err(InsertError::LoginnameConflict.into_rpc_error())
        } else {
            let id = loop {
                let id = uuid::Uuid::new_v4();
                if !db.contains_id(id) {
                    break id;
                }
            };

            let info = FullUserInfo {
                loginname,
                real_name,
                password_hash,
            };

            let op = DatabaseOperation::Insert(id, info);

            let mut lock = self.replication_state.write().unwrap();
            lock.add_entry(op, db);

            Ok(id)
        }
    }

    fn modify(
        &self,
        oldloginname: String,
        newloginname: String,
        password: Option<String>,
    ) -> Result<(), jsonrpc_core::Error> {
        let mut lock = self.db.write().unwrap();
        let db = &mut *lock;

        leader_or_err(&self.replication_state.read().unwrap())?;

        match db.get_by_loginname(&oldloginname) {
            None => Err(jsonrpc_core::Error::invalid_params(
                "No such existing loginname",
            )),
            Some((id, info)) => {
                if check_password(password.as_deref(), &info.password_hash)
                    .map_err(to_internal_error)?
                {
                    if db.contains_loginname(&newloginname) {
                        Err(InsertError::LoginnameConflict.into_rpc_error())
                    } else {
                        let mut lock = self.replication_state.write().unwrap();
                        lock.add_entry(DatabaseOperation::ChangeLoginname(id, newloginname), db);

                        Ok(())
                    }
                } else {
                    Err(jsonrpc_core::Error::invalid_params("Incorrect password"))
                }
            }
        }
    }

    fn delete(
        &self,
        loginname: String,
        password: Option<String>,
    ) -> Result<(), jsonrpc_core::Error> {
        let mut lock = self.db.write().unwrap();
        let db = &mut *lock;

        leader_or_err(&self.replication_state.read().unwrap())?;

        match db.get_by_loginname(&loginname) {
            None => Err(jsonrpc_core::Error::invalid_params(
                "No such existing loginname",
            )),
            Some((id, info)) => {
                if check_password(password.as_deref(), &info.password_hash)
                    .map_err(to_internal_error)?
                {
                    let mut lock = self.replication_state.write().unwrap();
                    lock.add_entry(DatabaseOperation::Delete(id), db);
                    return Ok(());
                } else {
                    Err(jsonrpc_core::Error::invalid_params("Incorrect password"))
                }
            }
        }
    }

    fn get_names(&self) -> Result<Vec<String>, jsonrpc_core::Error> {
        let lock = self.db.read().unwrap();
        let db = &*lock;

        Ok(db
            .iter()
            .map(|(_, info)| info.loginname.to_owned())
            .collect())
    }

    fn get_uuids(&self) -> Result<Vec<uuid::Uuid>, jsonrpc_core::Error> {
        let lock = self.db.read().unwrap();
        let db = &*lock;

        Ok(db.iter().map(|(id, _)| *id).collect())
    }

    fn get_all(&self) -> Result<Vec<id_common::UserInfo>, jsonrpc_core::Error> {
        let lock = self.db.read().unwrap();
        let db = &*lock;

        Ok(db
            .iter()
            .map(|(id, info)| id_common::UserInfo {
                loginname: info.loginname.clone(),
                uuid: *id,
                real_name: info.real_name.clone(),
            })
            .collect())
    }
}

impl S2SRpc for Service {
    fn append_entries(
        &self,
        key: String,
        term: TermID,
        leader_id: std::net::SocketAddr,
        prev_log_index: u64,
        prev_log_term: TermID,
        entries: Vec<ReplicationLogEntry>,
    ) -> Result<AppendEntriesResult, jsonrpc_core::Error> {
        if key != REPLICATION_KEY {
            return Err(jsonrpc_core::Error::invalid_params("not allowed"));
        }

        let mut state_lock = self.replication_state.write().unwrap();
        let state = &mut *state_lock;

        if term < state.current_term.id {
            return Ok(AppendEntriesResult::Failure {
                term: state.current_term.id,
            });
        }

        let _ = self.replication_ping_recv_send.try_send(());

        println!("received ping from {}", leader_id);

        state.leader_id = Some(leader_id);
        if term > state.current_term.id {
            state.current_term = CurrentTermState::new(term);

            state.role = ReplicationRoleState::Follower;
        }

        let prev_log_matches = if prev_log_index == 0 {
            true
        } else if prev_log_index < state.log_start {
            true // TODO maybe check
        } else if let Some(entry) = state.log.get((prev_log_index - state.log_start) as usize) {
            entry.term == prev_log_term
        } else {
            false
        };

        if !prev_log_matches {
            return Ok(AppendEntriesResult::Failure {
                term: state.current_term.id,
            });
        }

        if (state.log_start + (state.log.len() as u64) - 1) != prev_log_index {
            // TODO recovery/commits

            panic!(
                "inconsistent replication state ({} {} {})",
                state.log_start,
                state.log.len(),
                prev_log_index
            );
        }

        if !entries.is_empty() {
            let mut db_lock = self.db.write().unwrap();
            let db = &mut *db_lock;

            for entry in entries {
                db.apply(entry.operation.clone());
                state.log.push(entry);
            }
        }

        Ok(AppendEntriesResult::Success)
    }

    fn request_vote(
        &self,
        key: String,
        term: TermID,
        candidate_id: std::net::SocketAddr,
        last_log_index: u64,
        last_log_term: TermID,
    ) -> Result<RequestVoteResult, jsonrpc_core::Error> {
        if key != REPLICATION_KEY {
            return Err(jsonrpc_core::Error::invalid_params("not allowed"));
        }

        let mut state_lock = self.replication_state.write().unwrap();
        let state = &mut *state_lock;

        let grant_vote = if term < state.current_term.id {
            false
        } else {
            if term > state.current_term.id {
                state.current_term = CurrentTermState::new(term);
                state.role = ReplicationRoleState::Follower;
            }
            if state.current_term.voted_for.is_some()
                || last_log_index < (state.log_start + state.log.len() as u64 - 1)
            {
                false
            } else {
                if last_log_index == (state.log_start + (state.log.len() as u64) - 1) {
                    last_log_index == 0 || state.log.last().unwrap().term < last_log_term
                } else {
                    true
                }
            }
        };

        if grant_vote {
            println!("voting for {}", candidate_id);
            state.current_term.voted_for = Some(candidate_id);

            Ok(RequestVoteResult {
                term: state.current_term.id,
                vote_granted: true,
            })
        } else {
            println!(
                "denying vote {} {} {}",
                last_log_index,
                state.log_start,
                state.log.len()
            );
            Ok(RequestVoteResult {
                term: state.current_term.id,
                vote_granted: false,
            })
        }
    }

    fn install_snapshot(
        &self,
        key: String,
        term: TermID,
        leader_id: std::net::SocketAddr,
        last_included_index: u64,
        _last_included_term: TermID,
        data: String,
    ) -> Result<TermID, jsonrpc_core::Error> {
        if key != REPLICATION_KEY {
            return Err(jsonrpc_core::Error::invalid_params("not allowed"));
        }

        let mut db_lock = self.db.write().unwrap();
        let db = &mut *db_lock;

        let mut state_lock = self.replication_state.write().unwrap();
        let state = &mut *state_lock;

        if term < state.current_term.id {
            Ok(state.current_term.id)
        } else {
            let _ = self.replication_ping_recv_send.try_send(());

            if term > state.current_term.id {
                state.current_term = CurrentTermState::new(term);
                state.role = ReplicationRoleState::Follower;
                state.leader_id = Some(leader_id);
            }

            if last_included_index >= state.log_start + (state.log.len() as u64) {
                let snapshot: Snapshot = serde_json::from_str(&data).map_err(to_internal_error)?;
                let new_db =
                    Database::load_from_snapshot(snapshot.clone()).map_err(to_internal_error)?;
                *db = new_db;
                state.log.clear();
                state.log_start = last_included_index + 1;
                state.last_snapshot = Some((snapshot, data));
            }

            Ok(state.current_term.id)
        }
    }
}

///Time interval between file save
const SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

fn main() {
    let pkey_str = include_bytes!("../certprivkey");

    let pkey = rustls::PrivateKey(
        rustls_pemfile::pkcs8_private_keys(&mut &pkey_str[..])
            .unwrap()
            .pop()
            .unwrap(),
    );

    let certificates = vec![rustls::Certificate(
        include_bytes!("../../certificate").to_vec(),
    )];

    let tls_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(certificates, pkey)
            .unwrap(),
    );

    #[derive(clap::Parser)]
    struct Args {
        #[clap(long, default_value = "5181")]
        numport: u16,

        #[clap(long)]
        peer: Vec<std::net::SocketAddr>,
    }

    let args: Args = clap::Parser::parse();

    let port = args.numport;

    let my_ip = local_ip_address::local_ip().unwrap();
    let my_address: std::net::SocketAddr = (my_ip, port).into();

    let (replication_ping_recv_send, replication_ping_recv_recv) = tokio::sync::mpsc::channel(1);

    let node_list = {
        let mut res = args.peer;
        if !res.contains(&my_address) {
            println!("adding my own address ({}) as peer", my_address);
            res.push(my_address);
        }

        res
    };

    let rt = tokio::runtime::Runtime::new().unwrap();

    let node_list = {
        let tls_connector: tokio_native_tls::TlsConnector =
            tokio_native_tls::native_tls::TlsConnector::builder()
                // this whole setup is already insecure, may as well ignore the cert
                .danger_accept_invalid_certs(true)
                // Workaround for https://github.com/briansmith/webpki/issues/54
                .use_sni(false)
                .build()
                .unwrap()
                .into();

        let http_client = hyper::Client::builder().build(hyper_tls::HttpsConnector::from((
            {
                let mut connector = hyper::client::HttpConnector::new();
                connector.enforce_http(false);
                connector
            },
            tls_connector.into(),
        )));

        node_list
            .into_iter()
            .map(|addr| {
                let client: S2SRpcClient = rt
                    .block_on(jsonrpc_core_client::transports::http::connect_with_client(
                        &format!("https://{}", addr),
                        http_client.clone(),
                    ))
                    .unwrap();

                (addr, client)
            })
            .collect()
    };

    let mut replication_state = ReplicationState::new(my_address, node_list);

    let server_id_hash = {
        use sha2::Digest;

        let mut hasher = sha2::Sha224::new();
        hasher.update(my_address.to_string());
        let result = hasher.finalize();
        base64::encode_config(result, base64::URL_SAFE)
    };

    let snapshot_path = format!("snapshot_{}.json", server_id_hash);
    let snapshot_path = std::path::PathBuf::from(snapshot_path);

    let db = match std::fs::File::open(&snapshot_path) {
        Ok(mut src) => {
            use std::io::Read;

            let mut snapshot_str = String::new();
            src.read_to_string(&mut snapshot_str)
                .expect("Failed to load snapshot");
            let snapshot: Snapshot =
                serde_json::from_str(&snapshot_str).expect("Failed to load snapshot");

            let db =
                Database::load_from_snapshot(snapshot.clone()).expect("Failed to load snapshot");
            replication_state.log_start = snapshot.last_included_index + 1;
            replication_state.current_term = CurrentTermState::new(snapshot.last_included_term);
            replication_state.last_snapshot = Some((snapshot, snapshot_str));

            db
        }
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                Database::new()
            } else {
                panic!("Failed to load snapshot: {:?}", err);
            }
        }
    };

    let db = Arc::new(RwLock::new(db));

    let replication_state = Arc::new(RwLock::new(replication_state));

    let service = Service {
        db: db.clone(),
        replication_state: replication_state.clone(),
        replication_ping_recv_send: replication_ping_recv_send.clone(),
    };

    let mut io = jsonrpc_core::IoHandler::new();
    io.extend_with(id_common::Rpc::to_delegate(service.clone()));
    io.extend_with(S2SRpc::to_delegate(service));

    let io = jsonrpc_core::MetaIoHandler::from(io);

    let server = jsonrpc_http_server::ServerBuilder::new(io)
        .start_https(
            &std::net::SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, port, 0, 0).into(),
            tls_config,
        )
        .expect("Failed to start server");

    let (stop_tx, stop_rx) = std::sync::mpsc::channel();

    rt.spawn(run_replication_task(
        replication_state.clone(),
        replication_ping_recv_recv,
        replication_ping_recv_send,
    ));

    std::thread::spawn(move || {
        let dest = atomicwrites::AtomicFile::new(
            snapshot_path,
            atomicwrites::OverwriteBehavior::AllowOverwrite,
        );
        loop {
            let stop = match stop_rx.recv_timeout(SAVE_INTERVAL) {
                Ok(()) => true,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("Lost stop handler");
                }
            };

            let db_lock = db.read().unwrap();
            let state_lock = replication_state.write().unwrap();

            if !state_lock.log.is_empty() {
                let snapshot = Snapshot {
                    last_included_index: state_lock.log_start + (state_lock.log.len() as u64) - 1,
                    last_included_term: state_lock.log.last().unwrap().term,
                    id_map: db_lock.id_map.clone(),
                };

                let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();

                if let Err(err) = dest.write(|dest| {
                    use std::io::Write;
                    dest.write_all(&snapshot_bytes)
                }) {
                    eprintln!("Failed to persist DB: {:?}", err);
                }
            }

            if stop {
                std::mem::drop(dest);
                std::process::exit(0);
            }
        }
    });

    ctrlc::set_handler(move || {
        if let Err(err) = stop_tx.send(()) {
            eprintln!("{:?}", err);
            std::process::exit(1);
        }
    })
    .unwrap();

    server.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use id_common::Rpc;

    fn init_test_service() -> Service {
        Service {
            db: Arc::new(RwLock::new(Database::new())),
        }
    }

    #[test]
    fn db_insert_conflict() {
        let mut db = Database::new();

        let loginname = "trisha";

        db.insert(FullUserInfo {
            loginname: loginname.to_owned(),
            real_name: None,
            password_hash: None,
        })
        .unwrap();

        assert_eq!(
            db.insert(FullUserInfo {
                loginname: loginname.to_owned(),
                real_name: None,
                password_hash: None,
            }),
            Err(InsertError::LoginnameConflict)
        );
    }

    #[test]
    fn db_get_by_id() {
        let mut db = Database::new();

        let info = FullUserInfo {
            loginname: "tom".to_owned(),
            real_name: None,
            password_hash: None,
        };

        let id = db.insert(info.clone()).unwrap();

        assert_eq!(db.get_by_id(id), Some(&info));
    }

    #[test]
    fn db_get_by_loginname() {
        let mut db = Database::new();

        let loginname = "tim";

        let info = FullUserInfo {
            loginname: loginname.to_owned(),
            real_name: None,
            password_hash: None,
        };

        let id = db.insert(info.clone()).unwrap();

        assert_eq!(db.get_by_loginname(loginname), Some((id, &info)));
    }

    #[test]
    fn db_delete() {
        let mut db = Database::new();

        let loginname = "tyler";

        let id = db
            .insert(FullUserInfo {
                loginname: loginname.to_owned(),
                real_name: None,
                password_hash: None,
            })
            .unwrap();

        db.delete(id);

        assert_eq!(db.get_by_id(id), None);
        assert_eq!(db.get_by_loginname(loginname), None);
    }

    #[test]
    fn db_modify() {
        let mut db = Database::new();

        let old_loginname = "trevor";
        let new_loginname = "tristan";

        let id = db
            .insert(FullUserInfo {
                loginname: old_loginname.to_owned(),
                real_name: None,
                password_hash: None,
            })
            .unwrap();

        db.modify(id, new_loginname.to_owned()).unwrap();

        let new_info = FullUserInfo {
            loginname: new_loginname.to_owned(),
            real_name: None,
            password_hash: None,
        };

        assert_eq!(db.get_by_loginname(old_loginname), None);
        assert_eq!(db.get_by_loginname(new_loginname), Some((id, &new_info)));
        assert_eq!(db.get_by_id(id), Some(&new_info));
    }

    #[test]
    fn db_iter() {
        let mut db = Database::new();

        let info = FullUserInfo {
            loginname: "tanner".to_owned(),
            real_name: None,
            password_hash: None,
        };

        let id = db.insert(info.clone()).unwrap();

        assert_eq!(db.iter().next(), Some((&id, &info)));
    }

    #[test]
    fn db_save_load() {
        use std::io::Seek;

        let mut db1 = Database::new();

        let info = FullUserInfo {
            loginname: "taylor".to_owned(),
            real_name: None,
            password_hash: None,
        };

        let id = db1.insert(info.clone()).unwrap();

        let mut file = tempfile::tempfile().unwrap();
        db1.save(&mut file).unwrap();

        file.rewind().unwrap();

        let db2 = Database::load(&mut file).unwrap();

        assert_eq!(db2.get_by_id(id), Some(&info));
    }

    #[test]
    fn rpc_lookup() {
        let service = init_test_service();

        let loginname = "trinity";

        let id = service.create(loginname.to_owned(), None, None).unwrap();

        assert_eq!(
            service.lookup(loginname.to_owned()),
            Ok(Some(id_common::UserInfo {
                uuid: id,
                loginname: loginname.to_owned(),
                real_name: None,
            }))
        );
    }

    #[test]
    fn rpc_lookup_none() {
        let service = init_test_service();

        assert_eq!(service.lookup("michael".to_owned()), Ok(None));
    }

    #[test]
    fn rpc_create_conflict() {
        let service = init_test_service();

        let loginname = "tessa";

        service.create(loginname.to_owned(), None, None).unwrap();

        assert!(service.create(loginname.to_owned(), None, None).is_err());
    }

    #[test]
    fn rpc_reverse_lookup() {
        let service = init_test_service();

        let loginname = "tara";

        let id = service.create(loginname.to_owned(), None, None).unwrap();

        assert_eq!(
            service.reverse_lookup(id),
            Ok(Some(id_common::UserInfo {
                uuid: id,
                loginname: loginname.to_owned(),
                real_name: None,
            }))
        );
    }

    #[test]
    fn rpc_reverse_lookup_none() {
        let service = init_test_service();

        assert_eq!(service.reverse_lookup(uuid::Uuid::new_v4()), Ok(None));
    }

    #[test]
    fn rpc_modify_wrong_password() {
        let service = init_test_service();

        let loginname = "talia";

        service
            .create(loginname.to_owned(), None, Some("iliketrains".to_owned()))
            .unwrap();

        assert!(service
            .modify(
                loginname.to_owned(),
                "olivia".to_owned(),
                Some("openbarley".to_owned())
            )
            .is_err());
    }

    #[test]
    fn rpc_modify_missing_password() {
        let service = init_test_service();

        let loginname = "tabitha";

        service
            .create(loginname.to_owned(), None, Some("hello".to_owned()))
            .unwrap();

        assert!(service
            .modify(loginname.to_owned(), "emma".to_owned(), None)
            .is_err());
    }

    #[test]
    fn rpc_modify_correct_password() {
        let service = init_test_service();

        let old_loginname = "tamara";
        let new_loginname = "teresa";
        let password = "treepowers";

        let id = service
            .create(old_loginname.to_owned(), None, Some(password.to_owned()))
            .unwrap();

        service
            .modify(
                old_loginname.to_owned(),
                new_loginname.to_owned(),
                Some(password.to_owned()),
            )
            .unwrap();

        let new_info = id_common::UserInfo {
            uuid: id,
            real_name: None,
            loginname: new_loginname.to_owned(),
        };

        assert_eq!(service.reverse_lookup(id).unwrap(), Some(new_info.clone()));
        assert_eq!(
            service.lookup(new_loginname.to_owned()).unwrap(),
            Some(new_info)
        );
        assert_eq!(service.lookup(old_loginname.to_owned()).unwrap(), None);
    }

    #[test]
    fn rpc_modify_no_password() {
        let service = init_test_service();

        let old_loginname = "tiana";
        let new_loginname = "tatum";

        let id = service
            .create(old_loginname.to_owned(), None, None)
            .unwrap();

        service
            .modify(old_loginname.to_owned(), new_loginname.to_owned(), None)
            .unwrap();

        let new_info = id_common::UserInfo {
            uuid: id,
            real_name: None,
            loginname: new_loginname.to_owned(),
        };

        assert_eq!(service.reverse_lookup(id).unwrap(), Some(new_info.clone()));
        assert_eq!(
            service.lookup(new_loginname.to_owned()).unwrap(),
            Some(new_info)
        );
        assert_eq!(service.lookup(old_loginname.to_owned()).unwrap(), None);
    }

    #[test]
    fn rpc_delete_wrong_password() {
        let service = init_test_service();

        let loginname = "tia";

        let id = service
            .create(loginname.to_owned(), None, Some("nollama".to_owned()))
            .unwrap();

        assert!(service
            .delete(loginname.to_owned(), Some("cornbreadandbeans".to_owned()))
            .is_err());
        assert!(service.lookup(loginname.to_owned()).unwrap().is_some());
        assert!(service.reverse_lookup(id).unwrap().is_some());
    }

    #[test]
    fn rpc_delete_missing_password() {
        let service = init_test_service();

        let loginname = "taryn";

        let id = service
            .create(
                loginname.to_owned(),
                None,
                Some("warningpointless".to_owned()),
            )
            .unwrap();

        assert!(service.delete(loginname.to_owned(), None).is_err());
        assert!(service.lookup(loginname.to_owned()).unwrap().is_some());
        assert!(service.reverse_lookup(id).unwrap().is_some());
    }

    #[test]
    fn rpc_delete_correct_password() {
        let service = init_test_service();

        let loginname = "travis";
        let password = "rainbows";

        let id = service
            .create(loginname.to_owned(), None, Some(password.to_owned()))
            .unwrap();

        service
            .delete(loginname.to_owned(), Some(password.to_owned()))
            .unwrap();
        assert_eq!(service.lookup(loginname.to_owned()), Ok(None));
        assert_eq!(service.reverse_lookup(id), Ok(None));
    }

    #[test]
    fn rpc_delete_no_password() {
        let service = init_test_service();

        let loginname = "trenton";

        let id = service.create(loginname.to_owned(), None, None).unwrap();

        service.delete(loginname.to_owned(), None).unwrap();
        assert_eq!(service.lookup(loginname.to_owned()), Ok(None));
        assert_eq!(service.reverse_lookup(id), Ok(None));
    }

    #[test]
    fn rpc_get_names() {
        let service = init_test_service();

        let loginname = "trey";

        service.create(loginname.to_owned(), None, None).unwrap();

        assert_eq!(service.get_names(), Ok(vec![loginname.to_owned()]));
    }

    #[test]
    fn rpc_get_uuids() {
        let service = init_test_service();

        let loginname = "troy";

        let id = service.create(loginname.to_owned(), None, None).unwrap();

        assert_eq!(service.get_uuids(), Ok(vec![id]));
    }

    #[test]
    fn rpc_get_all() {
        let service = init_test_service();

        let loginname = "tony";

        let id = service.create(loginname.to_owned(), None, None).unwrap();

        assert_eq!(
            service.get_all(),
            Ok(vec![id_common::UserInfo {
                uuid: id,
                real_name: None,
                loginname: loginname.to_owned(),
            }])
        );
    }
}
