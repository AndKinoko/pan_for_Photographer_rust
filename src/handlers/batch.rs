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

/// POST /api/batch/share 批量分享
pub async fn batch_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<BatchShareRequest>,
) -> Result<Json<Value>, AppError> {
    let total = req.file_ids.len();
    if total == 0 {
        return Err(AppError::BadRequest("请至少选择一个文件".into()));
    }
    if total > 500 {
        return Err(AppError::BadRequest(
            format!("单次操作最多 500 项，当前 {} 项", total),
        ));
    }

    let mut shares = Vec::new();
    let mut succeeded = 0;
    let mut failed = 0;

    for &file_id in &req.file_ids {
        let (file_name,): (String,) = match sqlx::query_as(
            "SELECT original_name FROM files WHERE id = ? AND owner_id = ?",
        )
        .bind(file_id)
        .bind(auth.user_id)
        .fetch_optional(&pool)
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => {
                failed += 1;
                shares.push(ShareItemResult {
                    file_id,
                    file_name: "(未知)".into(),
                    share_id: None,
                    share_url: None,
                    status: "failed".into(),
                });
                continue;
            }
            Err(_) => {
                failed += 1;
                shares.push(ShareItemResult {
                    file_id,
                    file_name: "(未知)".into(),
                    share_id: None,
                    share_url: None,
                    status: "failed".into(),
                });
                continue;
            }
        };

        match share_service::create_share(
            &pool,
            Some(file_id),
            None,
            auth.user_id,
            req.expires_hours,
            req.password.clone(),
            None,
            None,
        )
        .await
        {
            Ok(share) => {
                succeeded += 1;
                shares.push(ShareItemResult {
                    file_id,
                    file_name,
                    share_id: Some(share.id.clone()),
                    share_url: Some(format!("/share/{}", share.id)),
                    status: "created".into(),
                });
            }
            Err(e) => {
                failed += 1;
                shares.push(ShareItemResult {
                    file_id,
                    file_name,
                    share_id: None,
                    share_url: None,
                    status: format!("failed: {}", e.message()),
                });
            }
        }
    }

    Ok(Json(json!({
        "success": true,
        "data": BatchShareResult {
            total,
            succeeded,
            failed,
            shares,
        },
        "error": null
    })))
}

/// POST /api/batch/unshare 批量取消分享
pub async fn batch_unshare(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<BatchUnshareRequest>,
) -> Result<Json<Value>, AppError> {
    let total = req.file_ids.len();
    if total == 0 {
        return Err(AppError::BadRequest("请至少选择一个文件".into()));
    }
    // 数量上限与 batch_share 对齐。此前只有 batch_share 有限制，
    // batch_unshare 可传任意长度数组逐条串行查库——公网上这是个放大器。
    if total > batch_service::MAX_BATCH_SIZE {
        return Err(AppError::BadRequest(format!(
            "单次最多操作 {} 个文件",
            batch_service::MAX_BATCH_SIZE
        )));
    }

    let mut results = Vec::new();
    let mut unshared = 0;
    let mut failed = 0;

    for &file_id in &req.file_ids {
        let share: Option<(String, String)> = sqlx::query_as(
            "SELECT fs.id, f.original_name FROM file_shares fs JOIN files f ON fs.file_id = f.id WHERE fs.file_id = ? AND fs.owner_id = ? AND fs.is_active = 1",
        )
        .bind(file_id)
        .bind(auth.user_id)
        .fetch_optional(&pool)
        .await?;

        match share {
            Some((share_id, file_name)) => {
                sqlx::query("UPDATE file_shares SET is_active = 0 WHERE id = ?")
                    .bind(&share_id)
                    .execute(&pool)
                    .await?;
                unshared += 1;
                results.push(UnshareItemResult {
                    file_id,
                    file_name,
                    share_id: Some(share_id),
                    status: "unshared".into(),
                });
            }
            None => {
                // 取文件名用于结果输出。
                //
                // 这里必须带 owner_id：原实现是 `WHERE id = ?`，而 files.id 是
                // 稠密自增整数、公开接口又是已登录用户的 POST，
                // 于是「取消分享一个自己没有的分享」会回显**任意用户的文件名**。
                // 交付场景里文件名常含客户名与拍摄活动，这不是小泄漏。
                //
                // 查不到就返回占位符：既不区分「文件不存在」与「不属于你」，
                // 也不泄露任何一方的信息。
                let file_name: String = sqlx::query_scalar(
                    "SELECT original_name FROM files WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
                )
                .bind(file_id)
                .bind(auth.user_id)
                .fetch_optional(&pool)
                .await?
                .unwrap_or_else(|| "(未知)".to_string());

                failed += 1;
                results.push(UnshareItemResult {
                    file_id,
                    file_name,
                    share_id: None,
                    status: "not_found".into(),
                });
            }
        }
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