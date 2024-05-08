use std::{sync::OnceLock, time::Duration};

use binstalk_downloader::remote::{self, header::HeaderMap, StatusCode, Url};
use compact_str::CompactString;
use percent_encoding::{
    percent_decode_str, utf8_percent_encode, AsciiSet, PercentEncode, CONTROLS,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::to_string as to_json_string;
use tracing::debug;

use super::{GhApiError, GhGraphQLErrors};

pub(super) fn percent_encode_http_url_path(path: &str) -> PercentEncode<'_> {
    /// https://url.spec.whatwg.org/#fragment-percent-encode-set
    const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');

    /// https://url.spec.whatwg.org/#path-percent-encode-set
    const PATH: &AsciiSet = &FRAGMENT.add(b'#').add(b'?').add(b'{').add(b'}');

    const PATH_SEGMENT: &AsciiSet = &PATH.add(b'/').add(b'%');

    // The backslash (\) character is treated as a path separator in special URLs
    // so it needs to be additionally escaped in that case.
    //
    // http is considered to have special path.
    const SPECIAL_PATH_SEGMENT: &AsciiSet = &PATH_SEGMENT.add(b'\\');

    utf8_percent_encode(path, SPECIAL_PATH_SEGMENT)
}

pub(super) fn percent_decode_http_url_path(input: &str) -> CompactString {
    if input.contains('%') {
        percent_decode_str(input).decode_utf8_lossy().into()
    } else {
        // No '%', no need to decode.
        CompactString::new(input)
    }
}

pub(super) enum GhApiRet<T> {
    ReachedRateLimit { retry_after: Option<Duration> },
    NotFound,
    Success(T),
    Unauthorized,
}

pub(super) fn check_for_status<T>(status: StatusCode, headers: &HeaderMap) -> Option<GhApiRet<T>> {
    match status {
        remote::StatusCode::FORBIDDEN
            if headers
                .get("x-ratelimit-remaining")
                .map(|val| val == "0")
                .unwrap_or(false) =>
        {
            Some(GhApiRet::ReachedRateLimit {
                retry_after: headers.get("x-ratelimit-reset").and_then(|value| {
                    let secs = value.to_str().ok()?.parse().ok()?;
                    Some(Duration::from_secs(secs))
                }),
            })
        }

        remote::StatusCode::UNAUTHORIZED => Some(GhApiRet::Unauthorized),
        remote::StatusCode::NOT_FOUND => Some(GhApiRet::NotFound),

        _ => None,
    }
}

#[derive(Deserialize)]
enum GraphQLResponse<T> {
    #[serde(rename = "data")]
    Data(T),

    #[serde(rename = "errors")]
    Errors(GhGraphQLErrors),
}

#[derive(Serialize)]
struct GraphQLQuery {
    query: String,
}

fn get_graphql_endpoint() -> &'static Url {
    static GRAPHQL_ENDPOINT: OnceLock<Url> = OnceLock::new();

    GRAPHQL_ENDPOINT.get_or_init(|| {
        Url::parse("https://api.github.com/graphql").expect("Literal provided must be a valid url")
    })
}

pub(super) enum GraphQLResult<T, U> {
    Data(T),
    Else(GhApiRet<U>),
}

pub(super) async fn issue_graphql_query<T, U>(
    client: &remote::Client,
    query: String,
    auth_token: &str,
) -> Result<GraphQLResult<T, U>, GhApiError>
where
    T: DeserializeOwned,
{
    let graphql_endpoint = get_graphql_endpoint();

    let graphql_query = to_json_string(&GraphQLQuery { query }).map_err(remote::Error::from)?;

    debug!("Sending graphql query to {graphql_endpoint}: '{graphql_query}'");

    let request_builder = client
        .post(graphql_endpoint.clone(), graphql_query)
        .header("Accept", "application/vnd.github+json")
        .bearer_auth(&auth_token);

    let response = request_builder.send(false).await?;

    if let Some(ret) = check_for_status(response.status(), response.headers()) {
        return Ok(GraphQLResult::Else(ret));
    }

    let response: GraphQLResponse<T> = response.json().await?;

    match response {
        GraphQLResponse::Data(data) => Ok(GraphQLResult::Data(data)),
        GraphQLResponse::Errors(errors) if errors.is_rate_limited() => {
            Ok(GraphQLResult::Else(GhApiRet::ReachedRateLimit {
                retry_after: None,
            }))
        }
        GraphQLResponse::Errors(errors) => Err(errors.into()),
    }
}