use super::*;

impl DefaultReloginService {
    pub(super) async fn read_history(
        &self,
        id: &str,
        before: Option<&str>,
    ) -> Result<ReloginHistory, AdminError> {
        validate_ids(&[id.to_owned()])?;
        if before.is_some_and(|cursor| {
            cursor.len() != 32 || !cursor.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err(AdminError::invalid("重登历史游标不合法"));
        }
        if !self.entries().await?.iter().any(|entry| entry.id == id) {
            return Err(AdminError::not_found("重登资料不存在"));
        }
        let mut items = self
            .store()?
            .history(id, before, 51)
            .await
            .map_err(store_error)?;
        let next_cursor = if items.len() > 50 {
            items.truncate(50);
            items.last().map(|item| item.id.clone())
        } else {
            None
        };
        Ok(ReloginHistory { items, next_cursor })
    }

    pub(super) async fn read_totp(
        &self,
        id: &str,
        context: &MutationContext,
    ) -> Result<ReloginTotp, AdminError> {
        validate_ids(&[id.to_owned()])?;
        let entry = self
            .entries()
            .await?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| AdminError::not_found("重登资料不存在"))?;
        let secret = totp_secret(&entry.mfa_secret)
            .ok_or_else(|| AdminError::invalid("未保存有效的 2FA 密钥"))?;
        self.store()?
            .record_totp_access(id, context)
            .await
            .map_err(store_error)?;
        Ok(ReloginTotp { secret })
    }
}
