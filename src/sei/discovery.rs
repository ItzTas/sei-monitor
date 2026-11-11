use reqwest::header::ACCEPT;

use super::errors::{ErrorKind, SeiError};
use super::http::{Options, api_client, describe_transport, read_capped};
use super::wire::{ROUTE_VERSION, read_envelope};

const CANDIDATES: [&str; 2] = [
    "/sei/modulos/wssei/controlador_ws.php/api/v2",
    "/sei/modulos/mod-wssei/controlador_ws.php/api/v2",
];
const STATUS_THAT_READS_EVERY_ENVELOPE: u16 = 200;

enum Probe {
    Found,
    Unavailable(SeiError),
    Failed(SeiError),
    NotModule,
}

impl Probe {
    fn preference(&self) -> u8 {
        match self {
            Probe::Found => 0,
            Probe::Unavailable(_) => 1,
            Probe::Failed(_) => 2,
            Probe::NotModule => 3,
        }
    }
}

pub async fn discover(sei_url: &str, options: &Options) -> Result<String, SeiError> {
    let [first, second] = CANDIDATES.map(|path| format!("{sei_url}{path}"));
    let client = api_client(options);
    let (first_probe, second_probe) = tokio::join!(probe(&client, &first), probe(&client, &second));
    let (chosen, probe) = [(&first, first_probe), (&second, second_probe)]
        .into_iter()
        .min_by_key(|(_, probe)| probe.preference())
        .expect("two candidates were probed");
    match probe {
        Probe::Found => Ok(chosen.clone()),
        Probe::Unavailable(error) | Probe::Failed(error) => Err(error),
        Probe::NotModule => Err(SeiError::ModuleNotFound { first, second }),
    }
}

async fn probe(client: &reqwest::Client, base: &str) -> Probe {
    match answer(client, base).await {
        Ok((status, body)) => classify(status, &body),
        Err(error) => Probe::Failed(error),
    }
}

async fn answer(client: &reqwest::Client, base: &str) -> Result<(u16, Vec<u8>), SeiError> {
    let request = client
        .get(format!("{base}{ROUTE_VERSION}"))
        .header(ACCEPT, "application/json")
        .build()
        .map_err(|error| transport(format!("build the {ROUTE_VERSION} call"), error))?;
    let response = client
        .execute(request)
        .await
        .map_err(|error| transport("look for mod-wssei".to_owned(), error))?;
    let status = response.status().as_u16();
    let body = read_capped(response)
        .await
        .map_err(|error| SeiError::Decode {
            route: ROUTE_VERSION.to_owned(),
            detail: describe_transport(error),
        })?;
    Ok((status, body))
}

fn classify(status: u16, body: &[u8]) -> Probe {
    match read_envelope(ROUTE_VERSION, STATUS_THAT_READS_EVERY_ENVELOPE, body) {
        Ok(_) => Probe::Found,
        Err(SeiError::Api(mut error)) if error.kind == ErrorKind::ModuleUnavailable => {
            error.status = status;
            Probe::Unavailable(SeiError::Api(error))
        }
        Err(SeiError::Api(error)) if error.kind == ErrorKind::UnexpectedResponse => {
            Probe::NotModule
        }
        Err(_) => Probe::Found,
    }
}

fn transport(context: String, error: reqwest::Error) -> SeiError {
    SeiError::Transport {
        context,
        detail: describe_transport(error),
    }
}
