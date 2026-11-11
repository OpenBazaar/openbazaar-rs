//! The one test-network store a downloaded build can open without typing
//! the address. This is not a directory and not a DHT. A catalog read is
//! `GET /public/store` on that onion, which is already on the sale allowlist.

/// The Railway vendor's sale onion.
pub const TEST_NETWORK_ONION: &str = "gwyf5y2jzymphjmxatxjtivt5tow6oxtosjl53i2ylxr3wj365zourad.onion";

/// `http://` plus [`TEST_NETWORK_ONION`]. A clearnet host is not a seed.
pub(crate) fn test_network_seed() -> String {
    format!("http://{TEST_NETWORK_ONION}")
}

/// The Railway moderator's sale onion (`packaging/railway/moderator.Dockerfile`).
/// It holds the fixture moderator keys, which are public, so it is a testnet4
/// moderator only.
pub const TEST_NETWORK_MODERATOR_ONION: &str = "yc546t4ecasnoollz3voqrrqsevup3wk25g4d7mxaryfomtu7xh37ead.onion";

/// Where a testnet4 node sends a dispute letter unless `--moderator` names another node.
pub(crate) fn test_network_moderator() -> String {
    format!("http://{TEST_NETWORK_MODERATOR_ONION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_network_seed_is_the_vendor_onion_and_a_clearnet_sale_url_is_refused() {
        let seed = test_network_seed();
        assert_eq!(seed, format!("http://{TEST_NETWORK_ONION}"));
        assert_eq!(crate::replicas::normalize(&seed).unwrap(), seed);
        assert_eq!(crate::peer_http::sale_base(&seed).unwrap(), seed);
        assert!(crate::sale_gate::allowed("GET", "/public/store"));
        assert_eq!(
            format!("{seed}/public/store"),
            format!("http://{TEST_NETWORK_ONION}/public/store")
        );

        for clearnet in ["http://example.com", "https://shop.example", "http://203.0.113.5:8787"] {
            assert!(crate::replicas::normalize(clearnet).is_err(), "{clearnet}");
            assert!(crate::peer_http::sale_base(clearnet).is_err(), "{clearnet}");
        }
    }
}
