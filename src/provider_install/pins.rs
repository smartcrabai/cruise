use super::Provider;

// Keep in sync with action/scripts/provision-jcode.sh's MIN_JCODE_VERSION.
pub const JCODE_MIN_VERSION: &str = "0.88.0";

#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub version: &'static str,
    pub targets: &'static [TargetPin],
}
#[derive(Clone, Copy, Debug)]
pub struct TargetPin {
    pub key: &'static str,
    pub url: &'static str,
    pub archive: Archive,
    pub entry: &'static str,
    pub digest: Digest,
}
#[derive(Clone, Copy, Debug)]
pub enum Archive {
    Raw,
    TarGz { root: Option<&'static str> },
}
#[derive(Clone, Copy, Debug)]
pub enum Digest {
    Sha256(&'static str),
    Sha512Sri(&'static str),
}

macro_rules! j {
    ($key:literal, $hash:literal) => {
        TargetPin {
            key: $key,
            url: concat!(
                "https://github.com/1jehuang/jcode/releases/download/v0.99.1/jcode-",
                $key,
                ".tar.gz"
            ),
            archive: Archive::TarGz { root: None },
            entry: concat!("jcode-", $key),
            digest: Digest::Sha256($hash),
        }
    };
    ($key:literal, $hash:literal, windows) => {
        TargetPin {
            key: $key,
            url: concat!(
                "https://github.com/1jehuang/jcode/releases/download/v0.99.1/jcode-",
                $key,
                ".tar.gz"
            ),
            archive: Archive::TarGz { root: None },
            entry: concat!("jcode-", $key, ".exe"),
            digest: Digest::Sha256($hash),
        }
    };
}
macro_rules! c {
    ($key:literal, $hash:literal) => {
        TargetPin {
            key: $key,
            url: concat!(
                "https://downloads.claude.ai/claude-code-releases/2.1.250/",
                $key,
                "/claude"
            ),
            archive: Archive::Raw,
            entry: "claude",
            digest: Digest::Sha256($hash),
        }
    };
    ($key:literal, $hash:literal, windows) => {
        TargetPin {
            key: $key,
            url: concat!(
                "https://downloads.claude.ai/claude-code-releases/2.1.250/",
                $key,
                "/claude.exe"
            ),
            archive: Archive::Raw,
            entry: "claude.exe",
            digest: Digest::Sha256($hash),
        }
    };
}
macro_rules! o {
    ($key:literal, $hash:literal) => {
        TargetPin {
            key: $key,
            url: concat!(
                "https://registry.npmjs.org/@opencode/cli-",
                $key,
                "/-/cli-",
                $key,
                "-2.1.3.tgz"
            ),
            archive: Archive::TarGz {
                root: Some("package"),
            },
            entry: "package/bin/opencode",
            digest: Digest::Sha512Sri($hash),
        }
    };
    ($key:literal, $hash:literal, windows) => {
        TargetPin {
            key: $key,
            url: concat!(
                "https://registry.npmjs.org/@opencode/cli-",
                $key,
                "/-/cli-",
                $key,
                "-2.1.3.tgz"
            ),
            archive: Archive::TarGz {
                root: Some("package"),
            },
            entry: "package/bin/opencode.exe",
            digest: Digest::Sha512Sri($hash),
        }
    };
}
static JCODE: Pin = Pin {
    version: "0.99.1",
    targets: &[
        j!(
            "linux-x86_64",
            "0b2a612de97c6e01ae6de1c0cff555b3460cf8364d1a11d2f8649fc0c9d7a1e6"
        ),
        j!(
            "linux-aarch64",
            "f66ffbaf7c4c0124f45d6c8616571146deb8509b8beef2e8c0d76cc35dda9b91"
        ),
        j!(
            "macos-aarch64",
            "3623164d518b0c9d443ab07052084244b379e8a3377037db67d2382220f4ed14"
        ),
        j!(
            "macos-x86_64",
            "d7d53c40b1e105926e06df5aa02c0f082616d602bbd20e0c318248ce3fdad4c9"
        ),
        j!(
            "windows-x86_64",
            "c74f56b57df8df9b23f48a3e76382b3ca05d41c85fb7e8218319931a0560dcae",
            windows
        ),
        j!(
            "windows-aarch64",
            "ff44021ff1090b452f506c258544302e1a43c8d4ba7ccb8a469215569dee71b3",
            windows
        ),
    ],
};
static CLAUDE: Pin = Pin {
    version: "2.1.250",
    targets: &[
        c!(
            "darwin-arm64",
            "506d7362a9c625306044879a9d91d8f33bd2eef963681b56be3295db5fe3e34b"
        ),
        c!(
            "darwin-x64",
            "c247e9bf78fca8a081c8a746811d9641f4c38e727c341d253f359ddab6870f45"
        ),
        c!(
            "linux-x64",
            "2be252a00ac56e704d7fbf7e5e9ef1243584093334a861945238a0c27e84bdac"
        ),
        c!(
            "linux-arm64",
            "0f4ff542f895526499ce23d8752c64cbb2ceb05a32cf6a1c0d1e31d82b20643d"
        ),
        c!(
            "linux-x64-musl",
            "03d0022c591844a6c03f2acdb2015e8070aa68bb920d6a707d99e08dd726ccda"
        ),
        c!(
            "linux-arm64-musl",
            "7f2817dd6fc114dcd86a13706ecb290bf8985bc95dc1c079ee7836a1e3bea1d3"
        ),
        c!(
            "win32-x64",
            "63403e009e779b0788d1b46be8436e734075514e280bb1ee8e06e4126ba63f29",
            windows
        ),
        c!(
            "win32-arm64",
            "fca724738701840b662e6af20401ccae9d37986593cee7810dccd4adcc482677",
            windows
        ),
    ],
};
static OPENCODE: Pin = Pin {
    version: "2.1.3",
    targets: &[
        o!(
            "darwin-arm64",
            "sha512-EwunTL9Q/6GT+vnA5e9NJyLdFFHpj77bQI1JdyNNtSI7dKO/DHW2RlvKMty2uvONCTfA4zdkwop9GalLw5becg=="
        ),
        o!(
            "darwin-x64",
            "sha512-WOaOLjzodos6VdjwaoU4rf8s4cGw0y+nNGkffEqz/8/e3tAPx+qgNZdPda0Q0k74PVDEM99GSYFFgvCiSy2UUg=="
        ),
        o!(
            "darwin-x64-baseline",
            "sha512-vUTJQW9lU05RIKWPLGol8bhdlNg/HAJPuF0qkQ1nJpciO6gPIRlQn5LZFNLO2NlSNkKxPS6G6DEx3Bzu8MwdZQ=="
        ),
        o!(
            "linux-x64",
            "sha512-WVO7TebdgAcPnoOQ390ny3oyqpCH5MpPU76lHXml3G/wJz6QLxjzsQW2Lrabiewg046yAeKlf+1nPPZElF1qPw=="
        ),
        o!(
            "linux-x64-baseline",
            "sha512-8alfmvFW7woNP3plB/B16fhRESXXi0S92SUzF4INtfROE7tNJCUeLfje0q+DZ3Kzy7HRZXPXo3yapzFwrUhk9g=="
        ),
        o!(
            "linux-arm64",
            "sha512-Two8nCzLbQC5jZc5oAeize3hCPKSawa/tCm2N7s6EIqeEcdPU7VAQNYiyEkrC4UhDJk91EpplsizPg8XmHCdmA=="
        ),
        o!(
            "linux-x64-musl",
            "sha512-smzK7MfBqJgywgY28oQRG9/WHh7t0qYZlKj9r5oK5Wck7UkNhrenTUCZxxp3Vkbf9nvxez513ODinOCZ8EekAA=="
        ),
        o!(
            "linux-x64-baseline-musl",
            "sha512-XI3k+lcW70qTCZ/Wv70bsSIK5sP9WWQ/uwFL9XuLcrQ5o6Xt1z30daeJeNGONgLJdk3RNeEd3NQzzM2mUziSgQ=="
        ),
        o!(
            "linux-arm64-musl",
            "sha512-YGMtt8NPszzj+hC/6Jn4iey8CT8Q1qk+LBiargFTDSW2cL7mZlo7rBfXnOAG2i4Xig1a25/1DyiNvGzoJ9TqYg=="
        ),
        o!(
            "windows-x64",
            "sha512-McB1yog8ZPZEkfOIhaPkjeAkvtRjAB5leq7oPzn0forJQk7OrtdXTKT+fPhYOVfrn71u/sAUqyW7NWRSm6Ce/g==",
            windows
        ),
        o!(
            "windows-x64-baseline",
            "sha512-7ttWmGr7KTACG8e+2o07ubrdzxnQwj2XHmEUm+rnAl+/5kmceko/2Jnscgq7rhWp/mF7otEvj3AfSrd6nPlv1Q==",
            windows
        ),
        o!(
            "windows-arm64",
            "sha512-k562H8RyaM6PHogVesqixzMv38Y3r0vGz9+GfgENx7kD3Umerg1DyCzg+Rle+ObavF5DqBIQWAaC7YGkp4nRfQ==",
            windows
        ),
    ],
};
#[must_use]
pub fn pin(provider: Provider) -> &'static Pin {
    match provider {
        Provider::Jcode => &JCODE,
        Provider::Claude => &CLAUDE,
        Provider::OpenCode => &OPENCODE,
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "isolated pin fixtures")]
mod tests {
    use super::*;

    #[test]
    fn pins_have_unique_keys_and_well_formed_digests() {
        for (provider, count) in [
            (Provider::Jcode, 6),
            (Provider::Claude, 8),
            (Provider::OpenCode, 12),
        ] {
            let pin = pin(provider);
            assert_eq!(pin.targets.len(), count);
            let mut keys = std::collections::HashSet::new();
            for target in pin.targets {
                assert!(keys.insert(target.key));
                assert!(target.url.starts_with("https://"));
                match target.digest {
                    Digest::Sha256(hash) => {
                        assert!(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
                    }
                    Digest::Sha512Sri(sri) => {
                        assert!(sri.starts_with("sha512-") && sri.len() == 95);
                    }
                }
            }
        }
        let parse = |s: &str| {
            s.split('.')
                .map(|part| part.parse::<u32>().unwrap())
                .collect::<Vec<_>>()
        };
        assert!(parse(JCODE_MIN_VERSION) <= parse(pin(Provider::Jcode).version));
    }
}
