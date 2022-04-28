use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

///Structure used by server to store user information
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
struct FullUserInfo {
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

    ///Loads save state, returns error if fail to load, otherwise returns database
    pub fn load(src: &mut impl std::io::Read) -> Result<Self, LoadError> {
        let id_map: HashMap<uuid::Uuid, FullUserInfo> = serde_json::from_reader(src)?;
        let mut name_map: HashMap<String, uuid::Uuid> = Default::default();

        for (id, info) in id_map.iter() {
            if name_map.insert(info.loginname.clone(), *id).is_some() {
                return Err(LoadError::Integrity);
            }
        }

        Ok(Self { id_map, name_map })
    }

    ///Inserts new entry to database
    pub fn insert(&mut self, info: FullUserInfo) -> Result<uuid::Uuid, InsertError> {
        if self.name_map.contains_key(&info.loginname) {
            Err(InsertError::LoginnameConflict)
        } else {
            let id = loop {
                let id = uuid::Uuid::new_v4();
                if !self.id_map.contains_key(&id) {
                    break id;
                }
            };

            self.name_map.insert(info.loginname.clone(), id);
            self.id_map.insert(id, info);

            Ok(id)
        }
    }

    ///Removes entry from database based of id
    pub fn delete(&mut self, id: uuid::Uuid) {
        match self.id_map.get(&id) {
            None => {}
            Some(info) => {
                self.name_map.remove(&info.loginname);
                self.id_map.remove(&id);
            }
        }
    }

    ///Modifies entry in database based of id
    pub fn modify(&mut self, id: uuid::Uuid, newloginname: String) -> Result<(), InsertError> {
        if self.name_map.contains_key(&newloginname) {
            Err(InsertError::LoginnameConflict)
        } else {
            match self.id_map.get_mut(&id) {
                None => unimplemented!(),
                Some(info) => {
                    self.name_map.remove(&info.loginname);
                    self.name_map.insert(newloginname.to_string(), id);
                    info.loginname = newloginname;
                    return Ok(());
                }
            }
        }
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

    ///Saves database content to a file
    pub fn save(&self, dest: &mut impl std::io::Write) -> Result<(), serde_json::Error> {
        serde_json::to_writer(dest, &self.id_map)
    }

    ///Returns iterator to list all items in database
    pub fn iter(&self) -> impl Iterator<Item = (&uuid::Uuid, &FullUserInfo)> {
        self.id_map.iter()
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

///Implementation of RPC service
struct Service {
    db: Arc<RwLock<Database>>,
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
        let mut lock = self.db.write().unwrap();
        let db = &mut *lock;

        let password_hash = password
            .map(|password| bcrypt::hash(password, bcrypt::DEFAULT_COST))
            .transpose()
            .map_err(to_internal_error)?;

        db.insert(FullUserInfo {
            loginname,
            real_name,
            password_hash,
        })
        .map_err(InsertError::into_rpc_error)
    }

    fn modify(
        &self,
        oldloginname: String,
        newloginname: String,
        password: Option<String>,
    ) -> Result<(), jsonrpc_core::Error> {
        let mut lock = self.db.write().unwrap();
        let db = &mut *lock;
        match db.get_by_loginname(&oldloginname) {
            None => Err(jsonrpc_core::Error::invalid_params(
                "No such existing loginname",
            )),
            Some((id, info)) => {
                if check_password(password.as_deref(), &info.password_hash)
                    .map_err(to_internal_error)?
                {
                    db.modify(id, newloginname)
                        .map_err(|err| err.into_rpc_error())
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

        match db.get_by_loginname(&loginname) {
            None => Err(jsonrpc_core::Error::invalid_params(
                "No such existing loginname",
            )),
            Some((id, info)) => {
                if check_password(password.as_deref(), &info.password_hash)
                    .map_err(to_internal_error)?
                {
                    db.delete(id);
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
    }

    let args: Args = clap::Parser::parse();

    let port = args.numport;

    let db_path = std::path::Path::new("db.json");

    let db = match std::fs::File::open(&db_path) {
        Ok(mut src) => Database::load(&mut src).expect("Failed to load database"),
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                Database::new()
            } else {
                panic!("Failed to load database: {:?}", err);
            }
        }
    };

    let db = Arc::new(RwLock::new(db));

    let mut io = jsonrpc_core::IoHandler::new();
    io.extend_with(id_common::Rpc::to_delegate(Service { db: db.clone() }));

    let io = jsonrpc_core::MetaIoHandler::from(io);

    let server = jsonrpc_http_server::ServerBuilder::new(io)
        .start_https(
            &std::net::SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, port, 0, 0).into(),
            tls_config,
        )
        .expect("Failed to start server");

    let (stop_tx, stop_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let dest =
            atomicwrites::AtomicFile::new(db_path, atomicwrites::OverwriteBehavior::AllowOverwrite);
        loop {
            let stop = match stop_rx.recv_timeout(SAVE_INTERVAL) {
                Ok(()) => true,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("Lost stop handler");
                }
            };
            if let Err(err) = dest.write(|dest| db.read().unwrap().save(dest)) {
                eprintln!("Failed to persist DB: {:?}", err);
            }

            if stop {
                std::mem::drop(dest);
                std::process::exit(0);
            }
        }
    });

    ctrlc::set_handler(move || {
        stop_tx.send(()).unwrap();
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
