#[tokio::main]
async fn main() {
    #[derive(Clone, clap::ArgEnum)]
    enum GetKind {
        Users,
        Uuids,
        All,
    }

    ///Queries client can use on id_server and associated arguments
    #[derive(clap::Subcommand)]
    enum Query {
        #[clap(name = "--lookup")]
        Lookup { loginname: String },

        #[clap(name = "--reverse-lookup")]
        ReverseLookup { uuid: uuid::Uuid },

        #[clap(name = "--get")]
        Get {
            #[clap(arg_enum)]
            kind: GetKind,
        },

        #[clap(name = "--create")]
        Create {
            loginname: String,
            real_name: Option<String>,
            #[clap(long)]
            password: Option<String>,
        },

        #[clap(name = "--delete")]
        Delete {
            loginname: String,
            #[clap(long)]
            password: Option<String>,
        },

        #[clap(name = "--modify")]
        Modify {
            oldloginname: String,
            newloginname: String,
            #[clap(long)]
            password: Option<String>,
        },
    }

    ///Information for all commands to server
    #[derive(clap::Parser)]
    struct Args {
        #[clap(subcommand)]
        query: Query,

        #[clap(long, default_value = "localhost")]
        server: String,

        #[clap(long, default_value = "5181")]
        numport: u16,
    }

    let args: Args = clap::Parser::parse();

    let tls_connector: tokio_native_tls::TlsConnector =
        tokio_native_tls::native_tls::TlsConnector::builder()
            // this whole setup is already insecure, may as well ignore the cert
            .danger_accept_invalid_certs(true)
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

    let client: id_common::RpcClient = jsonrpc_core_client::transports::http::connect_with_client(
        &format!("https://{}:{}", args.server, args.numport),
        http_client,
    )
    .await
    .unwrap();

    match args.query {
        Query::Lookup { loginname } => match client.lookup(loginname).await {
            Ok(None) => {
                println!("Not an existing login name.")
            }
            Ok(Some(userinfo)) => {
                println!("loginname: {}, uuid: {}", userinfo.loginname, userinfo.uuid);
                match userinfo.real_name {
                    None => {}
                    Some(real_name) => {
                        println!("real name: {}", real_name);
                    }
                }
            }
            Err(error) => {
                println!("Failed to look up, {:?}", error)
            }
        },

        Query::ReverseLookup { uuid } => match client.reverse_lookup(uuid).await {
            Ok(None) => {
                println!("Not an existing login name.")
            }
            Ok(Some(userinfo)) => {
                println!("loginname: {}, uuid: {}", userinfo.loginname, userinfo.uuid);
                match userinfo.real_name {
                    None => {}
                    Some(real_name) => {
                        println!("real name: {}", real_name);
                    }
                }
            }
            Err(error) => {
                println!("Failed to look up, {:?}", error)
            }
        },

        Query::Get { kind } => match kind {
            GetKind::Users => match client.get_names().await {
                Ok(users) => {
                    for user in users {
                        println!("loginname: {}", user);
                    }
                }
                Err(error) => {
                    println!("Failed to look up, {:?}", error)
                }
            },
            GetKind::Uuids => match client.get_uuids().await {
                Ok(users) => {
                    for user in users {
                        println!("uuid: {}", user);
                    }
                }
                Err(error) => {
                    println!("Failed to look up, {:?}", error)
                }
            },
            GetKind::All => match client.get_all().await {
                Ok(users) => {
                    for user in users {
                        println!("loginname: {}, uuid: {}", user.loginname, user.uuid);
                    }
                }
                Err(error) => {
                    println!("Failed to look up, {:?}", error)
                }
            },
        },
        Query::Create {
            loginname,
            real_name,
            password,
        } => match client.create(loginname, real_name, password).await {
            Ok(userid) => {
                println!("uuid: {}", userid);
            }
            Err(error) => {
                println!("Failed to create, {:?}", error)
            }
        },

        Query::Delete {
            loginname,
            password,
        } => match client.delete(loginname, password).await {
            Ok(()) => {
                println!("User deleted");
            }
            Err(error) => {
                println!("Failed to delete, {:?}", error)
            }
        },

        Query::Modify {
            oldloginname,
            newloginname,
            password,
        } => match client.modify(oldloginname, newloginname, password).await {
            Ok(()) => {
                println!("User login modified");
            }
            Err(error) => {
                println!("Failed to modify, {:?}", error)
            }
        },
    }
}
