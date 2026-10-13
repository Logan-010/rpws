pub struct NetApi {
    addr: String,
    client: reqwest::Client,
}

impl NetApi {
    pub fn new(addr: String) -> Self {
        Self {
            addr: addr.trim_end_matches('/').into(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn connect(&self) -> color_eyre::Result<()> {
        self.client
            .get(format!("{}/health", self.addr))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        Ok(())
    }

    pub async fn get<N: AsRef<str>>(
        &self,
        name: N,
    ) -> color_eyre::Result<Option<shared::RecordPayload>> {
        let response = self
            .client
            .post(format!("{}/get", self.addr))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .timeout(super::handlers::REQUEST_TIMEOUT + std::time::Duration::from_secs(5))
            .body(postcard::to_stdvec(name.as_ref())?)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = response.error_for_status()?.bytes().await?;
        Ok(Some(postcard::from_bytes(&body)?))
    }

    pub async fn put<N: AsRef<str>>(
        &self,
        name: N,
        payload: shared::RecordPayload,
    ) -> color_eyre::Result<()> {
        self.client
            .put(format!("{}/put", self.addr))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .timeout(super::handlers::REQUEST_TIMEOUT + std::time::Duration::from_secs(5))
            .body(postcard::to_stdvec(&(name.as_ref(), payload))?)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list(
        &self,
    ) -> color_eyre::Result<std::collections::HashMap<String, shared::RecordPayload>> {
        let body = self
            .client
            .get(format!("{}/list", self.addr))
            .timeout(super::handlers::REQUEST_TIMEOUT + std::time::Duration::from_secs(5))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(postcard::from_bytes(&body)?)
    }
}
