use crate::*;

#[near]
impl Contract {
    /// Changes the account allowed to transfer funds.
    pub fn set_spender(&mut self, spender_id: AccountId) {
        self.assert_admin();
        self.spender_id = spender_id;
    }

    /// Transfers the admin role to another account.
    pub fn set_admin(&mut self, admin_id: AccountId) {
        self.assert_admin();
        self.admin_id = admin_id;
    }
}

impl Contract {
    pub(crate) fn assert_admin(&self) {
        require!(
            env::predecessor_account_id() == self.admin_id,
            "Only the admin can call this method"
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;
    use near_sdk::AccountId;
    use near_sdk::testing_env;

    #[test]
    fn test_set_spender_and_admin() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        let new_spender: AccountId = "spender2.near".parse().unwrap();
        let new_admin: AccountId = "admin2.near".parse().unwrap();
        contract.set_spender(new_spender.clone());
        assert_eq!(contract.get_spender(), &new_spender);
        contract.set_admin(new_admin.clone());
        assert_eq!(contract.get_admin(), &new_admin);
        // The old admin lost control.
        testing_env!(context(admin()).build());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.set_spender(spender());
        }));
        assert!(result.is_err());
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_set_spender_by_spender_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.set_spender(spender());
    }
}
