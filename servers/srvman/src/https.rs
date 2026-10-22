use std::io;

use actix_web::{middleware, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};

/// simple handle
async fn index(req: HttpRequest) -> Result<HttpResponse, Error> {
    println!("{req:?}");
    Ok(HttpResponse::Ok()
        .content_type("text/json")
        .body(r#"{
            "this": 12,
            "that": [
                {
                    "obj": "obj1"
                },
                {
                    "obj": "obj2"
                }
            ]
        }"#))
}

#[actix_web::main]
pub async fn start(addr: &str) -> io::Result<()> {
    // env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // load TLS keys
    let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
    builder
        .set_private_key_file("secret/key.pem", SslFiletype::PEM)
        .unwrap();
    builder.set_certificate_chain_file("secret/cert.pem").unwrap();

    println!("starting HTTPS server at https://{addr}");

    HttpServer::new(|| {
        App::new()
            // enable logger
            .wrap(middleware::Logger::default())
            // register simple handler, handle all methods
            .service(web::resource("/index.html").to(index))
            // with path parameters
            .service(web::resource("/").route(web::get().to(|| async {
                HttpResponse::Found()
                    .append_header(("LOCATION", "/index.html"))
                    .finish()
            })))
    })
    .bind_openssl(addr, builder)?
    .run()
    .await
}