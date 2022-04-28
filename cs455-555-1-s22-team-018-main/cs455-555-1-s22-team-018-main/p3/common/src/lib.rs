use serde::{Deserialize, Serialize};

pub const NOT_LEADER_ERROR: i64 = -32005;

#[derive(Serialize, Deserialize)]
pub struct NotLeaderErrorData {
    pub leader_id: Option<std::net::SocketAddr>,
}

///RPC library structure for info related to user
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct UserInfo {
    pub loginname: String,
    pub uuid: uuid::Uuid,
    pub real_name: Option<String>,
}

///Remote functions clients can use on the server
#[jsonrpc_derive::rpc]
pub trait Rpc {
    ///Remote function to look up user information with login name
    #[rpc(name = "lookup")]
    fn lookup(&self, loginname: String) -> Result<Option<UserInfo>, jsonrpc_core::Error>;

    ///Remote function to create new user id with loginname, realname (optional), and password (optional)
    #[rpc(name = "create")]
    fn create(
        &self,
        loginname: String,
        realname: Option<String>,
        password: Option<String>,
    ) -> Result<uuid::Uuid, jsonrpc_core::Error>;

    ///Remote function to look up user information with uuid
    #[rpc(name = "reverse_lookup")]
    fn reverse_lookup(&self, uuid: uuid::Uuid) -> Result<Option<UserInfo>, jsonrpc_core::Error>;

    ///Remote function to modify login name of user id
    #[rpc(name = "modify")]
    fn modify(
        &self,
        oldloginname: String,
        newloginname: String,
        password: Option<String>,
    ) -> Result<(), jsonrpc_core::Error>;

    ///Remote function to delete user id info
    #[rpc(name = "delete")]
    fn delete(
        &self,
        loginname: String,
        password: Option<String>,
    ) -> Result<(), jsonrpc_core::Error>;

    ///Remote function to get login names of all users
    #[rpc(name = "get_names")]
    fn get_names(&self) -> Result<Vec<String>, jsonrpc_core::Error>;

    ///Remote function to get all uuid of all users
    #[rpc(name = "get_uuids")]
    fn get_uuids(&self) -> Result<Vec<uuid::Uuid>, jsonrpc_core::Error>;

    ///Remote function to get all login and real names of all users
    #[rpc(name = "get_all")]
    fn get_all(&self) -> Result<Vec<UserInfo>, jsonrpc_core::Error>;
}
