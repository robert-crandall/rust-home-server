use std::collections::BTreeMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfAccessConfig {
    pub team_domain: String,
    pub aud: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub database_url: String,
    pub listen_addr: SocketAddr,
    pub secure_cookies: bool,
    pub allow_open_registration: bool,
    pub cf_access: Option<CfAccessConfig>,
    pub household_dir: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let _ = dotenvy::dotenv();
        let vars: BTreeMap<String, String> = env::vars().collect();
        Self::from_map(&vars)
    }

    pub fn from_map(vars: &BTreeMap<String, String>) -> Result<Self, String> {
        let database_url = vars
            .get("DATABASE_URL")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| "DATABASE_URL is required".to_string())?
            .to_string();

        let raw_addr = vars
            .get("ADDR")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .unwrap_or(":8080");
        let listen_addr = parse_listen_addr(raw_addr)?;

        let secure_cookies = match vars
            .get("APP_ENV")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            None | Some("development") => false,
            Some("production") => true,
            Some(other) => {
                return Err(format!(
                    "APP_ENV must be 'development' or 'production' (got '{other}')"
                ));
            }
        };

        let allow_open_registration = match vars
            .get("ALLOW_OPEN_REGISTRATION")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            // Default to false so only the initial bootstrap account can register;
            // set to true to allow open multi-user registration.
            None => false,
            Some("true" | "1") => true,
            Some("false" | "0") => false,
            Some(other) => {
                return Err(format!(
                    "ALLOW_OPEN_REGISTRATION must be 'true' or 'false' (got '{other}')"
                ));
            }
        };

        let cf_team_domain = vars
            .get("CF_ACCESS_TEAM_DOMAIN")
            .map(|v| v.trim().trim_end_matches('/'))
            .filter(|v| !v.is_empty())
            .map(ToString::to_string);
        let cf_aud = vars
            .get("CF_ACCESS_AUD")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(ToString::to_string);

        let cf_access = match (cf_team_domain, cf_aud) {
            (Some(team_domain), Some(aud)) => {
                let parsed = url::Url::parse(&team_domain).map_err(|_| {
                    "CF_ACCESS_TEAM_DOMAIN must be a valid https:// URL".to_string()
                })?;
                if parsed.scheme() != "https" || parsed.host_str().is_none() {
                    return Err(
                        "CF_ACCESS_TEAM_DOMAIN must start with https:// and include a host"
                            .to_string(),
                    );
                }
                Some(CfAccessConfig { team_domain, aud })
            }
            (None, None) => None,
            _ => {
                return Err(
                    "CF_ACCESS_TEAM_DOMAIN and CF_ACCESS_AUD must either both be set or both be unset"
                        .to_string(),
                );
            }
        };

        let household_dir = vars
            .get("HOUSEHOLD_DIR")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);

        Ok(Self {
            database_url,
            listen_addr,
            secure_cookies,
            allow_open_registration,
            cf_access,
            household_dir,
        })
    }
}

pub fn parse_listen_addr(raw: &str) -> Result<SocketAddr, String> {
    let normalized = if let Some(port) = raw.strip_prefix(':') {
        format!("0.0.0.0:{port}")
    } else {
        raw.to_string()
    };
    normalized
        .parse::<SocketAddr>()
        .map_err(|err| format!("invalid ADDR '{raw}': {err}"))
}

pub fn healthcheck_url_from_env() -> Result<String, String> {
    let _ = dotenvy::dotenv();
    let raw_addr = env::var("ADDR").unwrap_or_else(|_| ":8080".to_string());
    healthcheck_url_for_addr(raw_addr.trim())
}

pub fn healthcheck_url_for_addr(raw_addr: &str) -> Result<String, String> {
    let addr = parse_listen_addr(if raw_addr.is_empty() {
        ":8080"
    } else {
        raw_addr
    })?;
    let host = match addr {
        SocketAddr::V4(v4) if v4.ip().is_unspecified() => "127.0.0.1".to_string(),
        SocketAddr::V4(v4) => v4.ip().to_string(),
        SocketAddr::V6(v6) if v6.ip().is_unspecified() => "[::1]".to_string(),
        SocketAddr::V6(v6) => format!("[{}]", v6.ip()),
    };
    Ok(format!("http://{host}:{}/healthz", addr.port()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shorthand_and_explicit_addrs() {
        assert_eq!(
            parse_listen_addr(":8080").unwrap().to_string(),
            "0.0.0.0:8080"
        );
        assert_eq!(
            parse_listen_addr("127.0.0.1:9090").unwrap().to_string(),
            "127.0.0.1:9090"
        );
        assert_eq!(
            healthcheck_url_for_addr(":8080").unwrap(),
            "http://127.0.0.1:8080/healthz"
        );
        assert_eq!(
            healthcheck_url_for_addr("[::]:3000").unwrap(),
            "http://[::1]:3000/healthz"
        );
    }

    #[test]
    fn validates_env_map() {
        let mut vars = BTreeMap::new();
        assert!(Config::from_map(&vars).is_err());

        vars.insert(
            "DATABASE_URL".into(),
            "postgres://localhost/rust_home_server".into(),
        );
        let cfg = Config::from_map(&vars).unwrap();
        assert!(!cfg.secure_cookies);
        assert!(!cfg.allow_open_registration);

        vars.insert("APP_ENV".into(), "production".into());
        vars.insert("ALLOW_OPEN_REGISTRATION".into(), "true".into());
        let prod = Config::from_map(&vars).unwrap();
        assert!(prod.secure_cookies);
        assert!(prod.allow_open_registration);

        vars.insert("APP_ENV".into(), "prod".into());
        assert!(Config::from_map(&vars).is_err());
    }

    #[test]
    fn validates_cloudflare_access_config() {
        let mut vars = BTreeMap::new();
        vars.insert(
            "DATABASE_URL".into(),
            "postgres://localhost/rust_home_server".into(),
        );
        vars.insert(
            "CF_ACCESS_TEAM_DOMAIN".into(),
            "https://example.cloudflareaccess.com/".into(),
        );
        assert!(Config::from_map(&vars).is_err());

        vars.insert("CF_ACCESS_AUD".into(), "aud-tag-123".into());
        let cfg = Config::from_map(&vars).unwrap();
        assert_eq!(
            cfg.cf_access,
            Some(CfAccessConfig {
                team_domain: "https://example.cloudflareaccess.com".into(),
                aud: "aud-tag-123".into(),
            })
        );

        vars.insert(
            "CF_ACCESS_TEAM_DOMAIN".into(),
            "http://example.cloudflareaccess.com".into(),
        );
        assert!(Config::from_map(&vars).is_err());
    }

    #[test]
    fn parses_optional_household_dir() {
        let mut vars = BTreeMap::new();
        vars.insert(
            "DATABASE_URL".into(),
            "postgres://localhost/rust_home_server".into(),
        );
        assert_eq!(Config::from_map(&vars).unwrap().household_dir, None);

        vars.insert("HOUSEHOLD_DIR".into(), "  /etc/household  ".into());
        assert_eq!(
            Config::from_map(&vars).unwrap().household_dir,
            Some(PathBuf::from("/etc/household"))
        );
    }
}
