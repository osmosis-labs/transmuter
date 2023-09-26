use super::TransmuterPool;

impl TransmuterPool {
    /// Check if the pool has the specified denom
    pub fn has_denom(&self, denom: &str) -> bool {
        self.pool_assets
            .iter()
            .any(|pool_asset| pool_asset.denom == denom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denom::Denom;

    #[test]
    fn test_has_denom() {
        let pool_assets = vec![
            Denom::unchecked("asset1"),
            Denom::unchecked("asset2"),
            Denom::unchecked("asset3"),
        ];
        let pool = TransmuterPool::new(&pool_assets).unwrap();

        assert!(pool.has_denom("asset1"));
        assert!(pool.has_denom("asset2"));
        assert!(pool.has_denom("asset3"));
        assert!(!pool.has_denom("asset4"));
    }
}
