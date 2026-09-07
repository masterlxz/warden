use serde_json::Value;

const DEFAULT_GRAPHQL_URL: &str = "https://arweave.net/graphql";
const DEFAULT_GATEWAY_BASE: &str = "https://arweave.net";

pub struct ArweaveClient {
    graphql_url: String,
    gateway_base: String,
    http: reqwest::Client,
}

impl ArweaveClient {
    pub fn new_default() -> Self {
        Self::new(DEFAULT_GRAPHQL_URL, DEFAULT_GATEWAY_BASE)
    }

    /// Points at a different gateway/GraphQL endpoint — used by tests to target a local fake
    /// gateway instead of the real Arweave network.
    pub fn new(graphql_url: impl Into<String>, gateway_base: impl Into<String>) -> Self {
        Self { graphql_url: graphql_url.into(), gateway_base: gateway_base.into(), http: reqwest::Client::new() }
    }

    /// Most recent transaction id published by `owner_address`, or `None` if that wallet has
    /// never published anything. There's no way to filter by app/tag (`pin()` only ever tags
    /// `App-Name: TruthID`), so "latest by this owner" is the only discovery mechanism available.
    pub async fn latest_tx_by_owner(&self, owner_address: &str) -> anyhow::Result<Option<String>> {
        let query = r#"query($owners:[String!]){ transactions(owners:$owners, first:1, sort:HEIGHT_DESC){ edges { node { id } } } }"#;
        let body = serde_json::json!({ "query": query, "variables": { "owners": [owner_address] } });
        let resp = self.http.post(&self.graphql_url).json(&body).send().await?;
        let text = resp.text().await?;
        Ok(parse_latest_tx_response(&text))
    }

    /// The wallet address that published `tx_id` — used once, right after this device's very
    /// first successful push, to learn the owner address to remember from then on.
    pub async fn owner_of_tx(&self, tx_id: &str) -> anyhow::Result<Option<String>> {
        let query = r#"query($id:ID!){ transaction(id:$id){ owner { address } } }"#;
        let body = serde_json::json!({ "query": query, "variables": { "id": tx_id } });
        let resp = self.http.post(&self.graphql_url).json(&body).send().await?;
        let text = resp.text().await?;
        Ok(parse_owner_response(&text))
    }

    /// Raw bytes stored at `tx_id` — our own already-encrypted bundle blob, since Arweave just
    /// stores whatever bytes the TruthID phone published on our behalf.
    pub async fn fetch_tx_data(&self, tx_id: &str) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/{}", self.gateway_base, tx_id);
        let resp = self.http.get(&url).send().await?.error_for_status()?;
        Ok(resp.bytes().await?.to_vec())
    }
}

fn parse_latest_tx_response(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    value
        .get("data")?
        .get("transactions")?
        .get("edges")?
        .as_array()?
        .first()?
        .get("node")?
        .get("id")?
        .as_str()
        .map(String::from)
}

fn parse_owner_response(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    value.get("data")?.get("transaction")?.get("owner")?.get("address")?.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_latest_tx_success_response() {
        let json = r#"{"data":{"transactions":{"edges":[{"node":{"id":"abc123"}}]}}}"#;
        assert_eq!(parse_latest_tx_response(json), Some("abc123".to_string()));
    }

    #[test]
    fn parses_a_real_shaped_no_results_response() {
        let json = r#"{"data":{"transactions":{"edges":[]}}}"#;
        assert_eq!(parse_latest_tx_response(json), None);
    }

    #[test]
    fn latest_tx_response_never_panics_on_malformed_json() {
        assert_eq!(parse_latest_tx_response("not json"), None);
        assert_eq!(parse_latest_tx_response(r#"{"errors":[{"message":"boom"}]}"#), None);
    }

    #[test]
    fn parses_a_real_shaped_owner_response() {
        let json = r#"{"data":{"transaction":{"owner":{"address":"wallet-abc"}}}}"#;
        assert_eq!(parse_owner_response(json), Some("wallet-abc".to_string()));
    }

    #[test]
    fn owner_response_never_panics_on_malformed_json() {
        assert_eq!(parse_owner_response("not json"), None);
        assert_eq!(parse_owner_response(r#"{"data":{"transaction":null}}"#), None);
    }
}
