use axum::{
    extract::State,
    Json,
};
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::batch::*;
use crate::services::batch_service;
use crate::services::share_service;
use sqlx::SqlitePool;

/// POST /api/batch/move 批量移动
pub async fn batch_move(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<BatchMoveCopyRequest>,
) -> Result<Json<Value>, AppError> {
    let result = batch_service::batch_move(&pool, auth.user_id, &req).await?;
    Ok(Json(json!({
        "success": true,
        "data": result,
        "error": null
    })))
}

/// POST /api/batch/copy 批量复制
pub async fn batch_copy(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<BatchMoveCopyRequest>,
) -> Result<Json<Value>, AppError> {
    let result = batch_service::batch_copy(&pool, auth.user_id, &req).await?;
    Ok(Json(json!({
        "success": true,
        "data": result,
        "error": null
    })))
}

/// POST /api/batch/delete 批量删除
pub async fn batch_delete(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<BatchDeleteRequest>,
) -> Result<Json<Value>, AppError> {
    let result = batch_service::batch_delete(&pool, auth.user_id, &req).await?;
    Ok(Json(json!({
        "success": true,
        "data": result,
        "error": null
    })))
}

/// POST /api/batch/unshare 批量取消分享
///
/// **批次模型下这个接口的语义变了。** 以前一个文件对应一个分享，所以
/// 「取消分享这个文件」是明确的。现在一个文件可能同时出现在好几个批次里，
/// 而分享是批次级的——没法只把其中一个文件从某个批次里摘掉。
///
/// 所以现在的行为是：**把这些项所在的、仍在生效的分享整体停用**。
/// 前端必须在确认框里把这一点说清楚（「这会同时停用 3 个分享链接」），
/// 否则用户会以为只取消了一个文件。
pub async fn batch_unshare(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<BatchUnshareRequest>,
) -> Result<Json<Value>, AppError> {
    let total = req.items.len();
    if total == 0 {
        return Err(AppError::BadRequest("请至少选择一个文件或文件夹".into()));
    }
    // 数量上限与其它批量操作对齐。没有它，一次可以传任意长度数组逐条查库——
    // 公网上这是个放大器。
    if total > batch_service::MAX_BATCH_SIZE {
        return Err(AppError::BadRequest(format!(
            "单次最多操作 {} 项",
            batch_service::MAX_BATCH_SIZE
        )));
    }

    // 先把类型收口成枚举。非法值直接拒绝，而不是静默跳过——
    // 「我选了 10 项，只处理了 8 项」是最难查的一类问题。
    let mut items: Vec<(crate::models::share::ItemKind, i64)> = Vec::with_capacity(total);
    for it in &req.items {
        let kind = it
            .kind()
            .ok_or_else(|| AppError::BadRequest("条目类型必须是 file 或 folder".into()))?;
        items.push((kind, it.id));
    }

    let touched = share_service::deactivate_shares_containing(&pool, auth.user_id, &items).await?;

    // 把 (item_id, 项名, share_id) 的平铺结果按条目归拢回一行一条
    let mut results: Vec<UnshareItemResult> = Vec::with_capacity(items.len());
    let mut unshared = 0usize;
    let mut failed = 0usize;

    for (kind, id) in &items {
        let mut hits = touched.iter().filter(|(tid, _, _)| tid == id);
        let Some((_, name, _)) = hits.next() else {
            // 没有任何活跃分享包含它。名字单独查一次，带上 owner_id——
            // 少了它，「取消一个自己没有的分享」会回显任意用户的文件名，
            // 而交付场景里文件名常含客户姓名与拍摄活动。
            let name_row: Option<(String,)> = if *kind == crate::models::share::ItemKind::File {
                sqlx::query_as("SELECT original_name FROM files WHERE id = ? AND owner_id = ?")
                    .bind(id)
                    .bind(auth.user_id)
                    .fetch_optional(&pool)
                    .await?
            } else {
                sqlx::query_as("SELECT name FROM folders WHERE id = ? AND owner_id = ?")
                    .bind(id)
                    .bind(auth.user_id)
                    .fetch_optional(&pool)
                    .await?
            };
            failed += 1;
            results.push(UnshareItemResult {
                id: *id,
                item_type: kind.as_str().to_string(),
                name: name_row
                    .map(|(n,)| n)
                    .unwrap_or_else(|| "(未知)".to_string()),
                share_ids: Vec::new(),
                status: "not_found".into(),
            });
            continue;
        };

        let share_ids: Vec<String> = touched
            .iter()
            .filter(|(tid, _, _)| tid == id)
            .map(|(_, _, sid)| sid.clone())
            .collect();
        unshared += 1;
        results.push(UnshareItemResult {
            id: *id,
            item_type: kind.as_str().to_string(),
            name: name.clone(),
            share_ids,
            status: "unshared".into(),
        });
    }

    Ok(Json(json!({
        "success": true,
        "data": BatchUnshareResult {
            total,
            unshared,
            failed,
            results,
        },
        "error": null
    })))
}
