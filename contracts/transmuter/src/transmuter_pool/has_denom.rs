use super::TransmuterPool;

impl TransmuterPool {
    /// Check if the pool has the specified denom
    pub fn has_denom(&self, denom: &str) -> bool {
        self.pool_assets
            .iter()
            .any(|pool_asset| pool_asset.denom() == denom)
    }
}

#[cfg(test)]
mod tests {
    use crate::asset::Asset;

    use super::*;

    #[test]
    fn test_has_denom() {
        let pool_assets = Asset::unchecked_equal_assets(&["asset1", "asset2", "asset3"]);
        let pool = TransmuterPool::new(pool_assets).unwrap();

        assert!(pool.has_denom("asset1"));
        assert!(pool.has_denom("asset2"));
        assert!(pool.has_denom("asset3"));
        assert!(!pool.has_denom("asset4"));
    }
}
