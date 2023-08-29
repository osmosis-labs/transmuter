use cw_storage_plus::Item;

use crate::{admin::Admin, shares::Shares, transmuter_pool::TransmuterPool};

pub const ACTIVE_STATUS: Item<bool> = Item::new("active_status");
pub const POOL: Item<TransmuterPool> = Item::new("pool");
pub const SHARES: Shares = Shares::new("share_denom");
pub const ADMIN: Admin = Admin::new("admin");

/// Referencing limiter for each pool asset denom.
/// This requires macro to avoid lifetime issue.
macro_rules! limiter {
    ($denom:expr) => {
        $crate::limiter::CompressedSMALimiter::new(
            $denom,
            &format!("window_config__{}", $denom),
            &format!("divisions__{}", $denom),
            &format!("boundary_offset__{}", $denom),
            &format!("latest_value__{}", $denom),
        )
    };
}
