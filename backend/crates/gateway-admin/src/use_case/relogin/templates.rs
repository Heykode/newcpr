use super::*;
use crate::{
    model::relogin_templates::validate_template_id,
    ports::store::{AdminStoreError, AdminStoreErrorKind},
};

pub(super) fn validate_selection(selection: &ReloginTemplateSelection) -> Result<(), AdminError> {
    validate_template_id(&selection.id)?;
    if selection.revision == 0 || selection.revision >= 9_007_199_254_740_991 {
        return Err(AdminError::invalid("模板版本不合法"));
    }
    Ok(())
}

pub(super) fn template_error(error: AdminStoreError) -> AdminError {
    match error.kind() {
        AdminStoreErrorKind::Invalid => {
            AdminError::invalid("模板分组或代理不可用，或模板数量已达上限，请检查模板")
        }
        AdminStoreErrorKind::Conflict | AdminStoreErrorKind::StaleRevision => {
            AdminError::conflict("模板名称重复或版本已变化，请刷新模板后重试")
        }
        _ => map_store_error(error, "relogin templates"),
    }
}

impl DefaultReloginService {
    pub(super) async fn resolve_template(
        &self,
        selection: Option<ReloginTemplateSelection>,
    ) -> Result<Option<ReloginTemplate>, AdminError> {
        let Some(selection) = selection else {
            return Ok(None);
        };
        validate_selection(&selection)?;
        let template = self
            .store()?
            .templates()
            .await
            .map_err(template_error)?
            .into_iter()
            .find(|template| template.id == selection.id)
            .ok_or_else(|| AdminError::conflict("模板已删除，请重新选择"))?;
        if template.revision != selection.revision {
            return Err(AdminError::conflict("模板已修改，请重新选择并确认配置"));
        }
        template.config.settings()?;
        Ok(Some(template))
    }
}
