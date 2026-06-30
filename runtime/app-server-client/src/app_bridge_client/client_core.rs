impl RemoteRuntimeClient {
    #[must_use]
    pub fn new(server_url: impl Into<String>) -> Self {
        Self {
            server_url: normalize_server_url(&server_url.into()),
            auth: RemoteAuth::default(),
            timeout: Duration::from_secs(5),
        }
    }

    #[must_use]
    pub fn with_auth(mut self, auth: RemoteAuth) -> Self {
        self.auth = auth;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    #[must_use]
    pub fn auth(&self) -> &RemoteAuth {
        &self.auth
    }

    pub fn health(&self) -> Result<Value, String> {
        self.json("GET", "/api/health", None)
    }

    pub fn models(&self) -> Result<Value, String> {
        self.json("GET", "/api/models", None)
    }

    pub fn agents(&self) -> Result<Value, String> {
        self.json("GET", "/api/agents", None)
    }
}
