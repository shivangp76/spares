use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::schema::undo::LatestEventResponse;
use spares_core::schema::undo::RedoEventRequest;
use spares_core::schema::undo::UndoEventRequest;

use crate::args::EventArgs;
use crate::args::EventCommands;
use crate::args::RedoArgs;
use crate::args::UndoArgs;
use crate::utils::ensure_ok;
use crate::utils::print_reverted;
use crate::utils::redo_event;
use crate::utils::undo_event;

pub(crate) async fn handle(
    event_args: EventArgs,
    base_url: &str,
    client: &Client,
) -> Result<(), Error> {
    match event_args.command {
        EventCommands::Latest => {
            let url = format!("{}/api/notes/latest-event-id", base_url);
            let response = client.get(url).send().await.map_err(|e| miette!("{}", e))?;
            let response = ensure_ok(response).await?;
            let response: LatestEventResponse =
                response.json().await.map_err(|e| miette!("{}", e))?;
            println!("{}", response.latest_event_id);
        }
        EventCommands::Undo(UndoArgs {
            event_id,
            undo_group,
        }) => {
            let request = UndoEventRequest {
                event_id,
                undo_group,
            };
            let undo_response_opt = undo_event(base_url, client, request)
                .await
                .map_err(|e| miette!("{}", e))?;
            match undo_response_opt {
                Some(undo_response) => {
                    print_reverted("Undone", &undo_response.undone_events);
                }
                None => {
                    println!("No event to undo");
                }
            }
        }
        EventCommands::Redo(RedoArgs {
            event_id,
            redo_group,
        }) => {
            let request = RedoEventRequest {
                event_id,
                redo_group,
            };
            let redo_response_opt = redo_event(base_url, client, request)
                .await
                .map_err(|e| miette!("{}", e))?;
            match redo_response_opt {
                Some(redo_response) => {
                    print_reverted("Redone", &redo_response.redone_events);
                }
                None => {
                    println!("No event to redo");
                }
            }
        }
    }
    Ok(())
}
