use anyhow::Result;
use std::{fs::File, io::Read, path::PathBuf};

pub async fn nats_client_from_opts(
    host: &str,
    port: &str,
    jwt: Option<String>,
    seed: Option<String>,
    credsfile: Option<PathBuf>,
) -> Result<async_nats::Client> {
    let nats_url = format!("{}:{}", host, port);
    use async_nats::ConnectOptions;

    let nc = if let Some(jwt_file) = jwt {
        let jwt_contents = extract_arg_value(&jwt_file)?;
        let kp = std::sync::Arc::new(if let Some(seed) = seed {
            nkeys::KeyPair::from_seed(&extract_arg_value(&seed)?)?
        } else {
            nkeys::KeyPair::new_user()
        });

        // You must provide the JWT via a closure
        async_nats::ConnectOptions::with_jwt(jwt_contents, move |nonce| {
            let key_pair = kp.clone();
            async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
        })
        .connect(&nats_url)
        .await?
    } else if let Some(credsfile_path) = credsfile {
        ConnectOptions::with_credentials_file(credsfile_path)
            .await?
            .connect(&nats_url)
            .await?
    } else {
        async_nats::connect(&nats_url).await?
    };
    Ok(nc)
}

fn extract_arg_value(arg: &str) -> Result<String> {
    match File::open(arg) {
        Ok(mut f) => {
            let mut value = String::new();
            f.read_to_string(&mut value)?;
            Ok(value)
        }
        Err(_) => Ok(arg.to_string()),
    }
}
