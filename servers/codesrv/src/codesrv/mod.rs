use com::{
    api,
    chrono::Utc,
    db::{self, Database},
    dylib_file_extension,
    mysql::{Transaction, PooledConn},
    net::{man::HeartBeat, *},
    status::Status, failed,
};
use crossbeam_channel::*;
use std::{fs::OpenOptions, io::Write, net::*, time::Duration};
use subprocess::{Exec, Redirection};

pub struct CodeSrv {
    pub conn: Connection,
    hb: Option<HeartBeat>,
    workspace: String,
    template_path: String,
    builder: String,
    conn_string: String,
    db: Option<PooledConn>,
}

unsafe impl Send for CodeSrv {}

impl Database for CodeSrv {
    fn db_conn(&mut self) -> &mut Option<PooledConn> {
        &mut self.db
    }

    fn db_conn_str(&self) -> &str {
        self.conn_string.as_str()
    }
}

impl CodeSrv {
    pub fn new(
        server: TcpListener,
        workspace: String,
        template_path: String,
        builder: String,
        db_addr: String,
        db_user: String,
        db_pass: String,
        srvman: Option<(String, usize)>,
    ) -> Result<Self, Status> {
        let conn_string = format!("mysql://{db_user}:{db_pass}@{db_addr}/eztrading");
        Ok(Self {
            conn: Self::connect(ServerConnection::AsServer(server), Handshake::CodeSrv(0))?,
            hb: if let Some(sys) = srvman {
                Some(HeartBeat::new(sys.0.as_str(), sys.1)?)
            } else {
                None
            },
            workspace,
            template_path,
            builder,
            db: Some(Self::new_pool(&conn_string)?),
            conn_string,
        })
    }

    pub fn build(
        txn: &mut Transaction,
        workspace: &String,
        template_path: &String,
        builder: &String,
        plugin_id: PluginId,
        job_id: JobId,
        publish: bool,
        price: Option<f32>
    ) -> Result<CodeSrvRep, CodeSrvError> {
        match db::Plugin::by_id(txn, plugin_id) {
            Ok(mut plugin) => {
                let root = format!("{workspace}/{}/{job_id}", plugin.user_id);
                let _ = std::fs::remove_dir_all(&root);
                if let Err(e) = std::fs::create_dir_all(&root) {
                    return Err(CodeSrvError::IoErr(failed!(e)));
                }
                let tmp_filename = format!("{root}/plugin_{plugin_id}.sm");
                let lib_filename = format!("{root}/plugin_{plugin_id}{}", dylib_file_extension());
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&tmp_filename)
                    .map_err(|e| CodeSrvError::IoErr(failed!(e)))?;
                // Write a slice of bytes to the file
                file.write_all(plugin.source.as_bytes())
                    .map_err(|e| CodeSrvError::IoErr(failed!(e)))?;

                let out = Exec::cmd(builder.as_str())
                    .arg(format!("{}/{job_id}", plugin.user_id))
                    .arg(tmp_filename)
                    .arg(workspace.as_str())
                    .arg(template_path.as_str())
                    .stdout(Redirection::Pipe)
                    .stderr(Redirection::Pipe)
                    .capture()
                    .map_err(|e| CodeSrvError::IoErr(failed!(e)))?;

                let res = if !out.exit_status.success() {
                    Ok(CodeSrvRep::Error(CodeSrvError::CompileError(
                        failed!(out.stderr_str()),
                    )))
                } else {
                    match std::fs::read(&lib_filename) {
                        Ok(lib) => {
                            plugin.lib = Some(lib);
                            plugin.published = publish;
                            if publish {
                                plugin.price = price;
                                plugin.date = Some(Utc::now().naive_utc().date());
                            }
                            if let Ok(cnt) = api::SchApi::test_plugin(lib_filename) {
                                plugin.machines = Some(cnt);
                            } else {
                                return Err(CodeSrvError::BuildError(failed!("Test failed")));
                            }
                            match plugin.upload(txn) {
                                Ok(p) => Ok(CodeSrvRep::Successful(p.id, p.name, p.machines.map(|v| v.machines).unwrap_or_default())),
                                Err(e) => Err(CodeSrvError::DbErr(e)),
                            }
                        }
                        Err(e) => Err(CodeSrvError::IoErr(failed!(e))),
                    }
                };
                let _ = std::fs::remove_dir_all(&root);
                res
            }
            Err(e) => Err(CodeSrvError::DbErr(e)),
        }
    }

    pub fn upload(
        txn: &mut Transaction,
        upload: Upload,
    ) -> Result<CodeSrvRep, CodeSrvError> {
        if let Some(pid) = upload.id {
            match db::Plugin::by_id(txn, pid) {
                Ok(mut plugin) => {
                    if plugin.published {
                        if upload
                            .version
                            .as_ref()
                            .map(|v| v.eq(&plugin.version))
                            .unwrap_or(true)
                        {
                            return Err(CodeSrvError::BuildError(failed!(format!(
                                "Plugin {} v{} was already published",
                                plugin.name, plugin.version
                            ))));
                        }
                        match db::Plugin::new(
                            txn,
                            plugin.user_id,
                            upload.name.unwrap_or("Untitled".into()),
                            upload.source.unwrap_or_default(),
                            upload.version.unwrap_or("1.0.0".into()),
                        ) {
                            Ok(p) => Ok(CodeSrvRep::Successful(p.id, p.name, p.machines.map(|v| v.machines).unwrap_or_default())),
                            Err(e) => Err(CodeSrvError::DbErr(e)),
                        }
                    } else {
                        if let Some(name) = upload.name {
                            plugin.name = name;
                        }
                        if let Some(src) = upload.source {
                            plugin.source = src;
                        }
                        if let Some(ver) = upload.version {
                            plugin.version = ver;
                        }
                        plugin.price = upload.price;
                        match plugin.upload(txn) {
                            Ok(p) => Ok(CodeSrvRep::Successful(p.id, p.name, p.machines.map(|v| v.machines).unwrap_or_default())),
                            Err(e) => Err(CodeSrvError::DbErr(e)),
                        }
                    }
                }
                Err(e) => Err(CodeSrvError::DbErr(e)),
            }
        } else if let Some(uid) = upload.user {
            match db::Plugin::new(
                txn,
                uid,
                upload.name.unwrap_or("Untitled".into()),
                upload.source.unwrap_or_default(),
                upload.version.unwrap_or("1.0.0".into()),
            ) {
                Ok(p) => Ok(CodeSrvRep::Successful(p.id, p.name, p.machines.map(|v| v.machines).unwrap_or_default())),
                Err(e) => Err(CodeSrvError::DbErr(e)),
            }
        } else {
            Err(CodeSrvError::DbErr(
                failed!("Neither user_id nor plugin_id is provided"),
            ))
        }
    }
}

impl Server for CodeSrv {
    type Error = CodeSrvError;
    type Reply = CodeSrvRep;
    type Request = CodeSrvReq;

    fn process(
        &mut self,
        req: CodeSrvReq,
        addr: SocketAddr,
        _: &Option<Sender<Packet>>,
    ) -> Result<Self::Reply, Self::Error> {
        let workspace = self.workspace.clone();
        let template_path = self.template_path.clone();
        let builder = self.builder.clone();
        match req {
            CodeSrvReq::Log(msg) => {
                println!("{}: {}", addr, msg);
                Ok(CodeSrvRep::Successful(0, "".into(), vec![]))
            }
            CodeSrvReq::Shutdown => Err(CodeSrvError::Shutdown),
            CodeSrvReq::Build(plugin_id, publish, price, job_id) => self.txn(|txn| {
                Self::build(
                    txn,
                    &workspace,
                    &template_path,
                    &builder,
                    plugin_id,
                    job_id,
                    publish,
                    price
                )
            }),
            CodeSrvReq::Upload(upload) => {
                self.txn(|txn| Self::upload(txn, upload))
            }
        }
    }

    fn shutdown(&mut self) {
        println!("Shutting down...");
        self.conn.disconnect().unwrap();
    }

    fn recv_timeout(
        &self,
    ) -> Result<(Packet, Option<Sender<Packet>>, SocketAddr), RecvTimeoutError> {
        self.conn.rx.recv_timeout(Duration::from_secs(1))
    }

    fn shutdown_signal() -> Self::Error {
        CodeSrvError::Shutdown
    }

    fn shutdown_request() -> Self::Request {
        CodeSrvReq::Shutdown
    }

    fn beat(&self) -> Result<(), Status> {
        if let Some(hb) = &self.hb {
            hb.beat()
        } else {
            Ok(())
        }
    }

    fn conn(&self) -> &Connection {
        &self.conn
    }
}
