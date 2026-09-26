pub async fn check_backend_health(http: &reqwest::Client, juiceback_url: &str) -> bool {
    let url = format!("{juiceback_url}/api/health");
    let fetch = async {
        let response = http.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response
            .json::<serde_json::Value>()
            .await
            .ok()?
            .get("status")?
            .as_str()
            .map(str::to_owned)
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), fetch)
        .await
        .ok()
        .flatten()
        .is_some_and(|status| status == "ok")
}
