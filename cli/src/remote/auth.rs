use super::*;

#[derive(Clone, Debug, Default)]
pub(crate) struct RemoteAuth {
    pub(super) token: Option<String>,
    pub(super) username: Option<String>,
    pub(super) password: Option<String>,
}

pub(crate) fn remote_auth_from_args(args: &[String]) -> RemoteAuth {
    RemoteAuth {
        token: value_for(args, &["--server-token"])
            .or_else(|| env::var(DEFAULT_SERVER_TOKEN_ENV).ok())
            .or_else(|| {
                value_for(args, &["--server-token-env"]).and_then(|name| env::var(name).ok())
            }),
        username: value_for(args, &["--username", "-u"]),
        password: value_for(args, &["--password", "-p"]),
    }
}
