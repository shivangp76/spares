use crate::adapters::impls::anki::ANKI_ADAPTER_NAME;
use crate::adapters::impls::anki::types::{
    AddFieldToModelApiRequestData, ApiAction, ApiRequest, ApiRequestParams, ModelName,
};
use crate::{AdapterErrorKind, Error, LibraryError};
use indicatif::ProgressIterator;
use reqwest::Client;
use serde_json::Value;

pub async fn execute_request(request: &ApiRequest, client: &Client) -> Result<Value, Error> {
    let api_url = "http://localhost:8765";
    let body = serde_json::to_string_pretty(&request).map_err(|e| {
        Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
            adapter_name: ANKI_ADAPTER_NAME.to_string(),
            error: e.to_string(),
        }))
    })?;
    // println!("{}", serde_json::to_string_pretty(&request).unwrap());
    let response = client.post(api_url).body(body).send().await.map_err(|e| {
        Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
            adapter_name: ANKI_ADAPTER_NAME.to_string(),
            error: format!("Failed to send the API request: {}", e),
        }))
    })?;
    if response.status().is_success() {
        let response_value = response.json::<Value>().await.map_err(|e| {
            Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: format!("Failed to get response body: {}", e),
            }))
        })?;
        // <https://git.foosoft.net/alex/anki-connect#sample-invocation>
        let response_result = response_value.get("result");
        if let Some(response) = response_result {
            return Ok(response.clone());
        }
        let response_error =
            response_value
                .get("error")
                .ok_or(Error::Library(LibraryError::Adapter(
                    AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: "Failed to get 'result'".to_string(),
                    },
                )))?;
        Err(Error::Library(LibraryError::Adapter(
            AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: format!(
                    "Failed to get 'result'. Got an 'error' of: {}",
                    response_error
                ),
            },
        )))
    } else {
        Err(Error::Library(LibraryError::Adapter(
            AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: format!("Request failed with status code: {}", response.status()),
            },
        )))
    }
}

pub async fn execute_requests(
    requests: &[ApiRequest],
    dry_run: bool,
    quiet: bool,
    client: &Client,
) -> Result<Vec<Value>, Error> {
    let mut results = Vec::new();
    for (i, request) in requests.iter().enumerate().progress() {
        if !dry_run {
            let result = execute_request(request, client).await?;
            if !quiet {
                println!("{}: {}", i, result);
            }
            results.push(result);
        }
    }
    Ok(results)
}

pub async fn create_field(field_name: &str, client: &Client) -> Result<(), Error> {
    let params = ApiRequestParams::AddFieldToModel(AddFieldToModelApiRequestData {
        model_name: ModelName::Basic,
        field_name: field_name.to_string(),
        index: None,
    });
    let api_request = ApiRequest {
        action: ApiAction::GetModelFieldNames,
        params,
        version: 6,
    };
    let _response = execute_request(&api_request, client).await?;
    Ok(())
}
