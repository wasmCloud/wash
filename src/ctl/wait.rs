use anyhow::{anyhow, bail, Result};
use cloudevents::{event::Event, AttributesReader};
use crossbeam_channel::Receiver;
use std::time::{Duration, Instant};

pub(crate) fn find_event<T>(
    receiver: &Receiver<Event>,
    timeout: Duration,
    check_function: impl Fn(Event) -> Result<Option<T>>,
) -> Result<T> {
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            bail!("Timeout waiting for event");
        }

        let event = receiver.recv_timeout(timeout - elapsed)?;

        let outcome = check_function(event)?;

        if let Some(result) = outcome {
            return Ok(result);
        }
    }
}

pub(crate) fn find_start_actor_result_event(
    host_id: String,
    actor_ref: String,
) -> impl Fn(Event) -> Result<Option<Result<()>>> {
    move |event: Event| {
        if event.source() != host_id.as_str() {
            return Ok(None);
        }

        let data: serde_json::Value = event
            .data()
            .ok_or(anyhow!("No data in event"))?
            .clone()
            .try_into()?;

        match event.ty() {
            "com.wasmcloud.lattice.actor_started" => {
                let image_ref = data
                    .get("image_ref")
                    .ok_or(anyhow!("No image_ref found in data"))?
                    .as_str()
                    .ok_or(anyhow!("image_ref is not a string"))?
                    .to_string();

                if image_ref == actor_ref {
                    return Ok(Some(Ok(())));
                }
            }
            "com.wasmcloud.lattice.actor_start_failed" => {
                let returned_actor_ref = data
                    .get("actor_ref")
                    .ok_or(anyhow!("No actor_ref found in data"))?
                    .as_str()
                    .ok_or(anyhow!("actor_ref is not a string"))?
                    .to_string();

                if returned_actor_ref == actor_ref {
                    let error = data
                        .get("error")
                        .ok_or(anyhow!("No error found in data"))?
                        .as_str()
                        .ok_or(anyhow!("error is not a string"))?
                        .to_string();

                    return Ok(Some(Err(anyhow!("Failed to start actor: {}", error))));
                }
            }
            _ => {}
        }

        Ok(None)
    }
}
