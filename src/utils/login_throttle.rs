//! 登录失败限流：按「用户名 + 来源 IP」双维度计数，指数退避。
//!
//! ## 为什么必须有
//!
//! 修复前 `/api/auth/login` 是一次裸查询 + 一次 `bcrypt::verify`，
//! 没有任何失败计数（全仓 grep `rate_limit|throttle|lockout` 零命中）。
//! 局域网内无所谓；一旦域名挂到公网，攻击者可以按字典无限爆破，
//! 而本项目是**自助注册**的摄影团队网盘——被撞库的账号能直接看到
//! 全部交付照片。
//!
//! ## 为什么按两个维度
//!
//! 只按 IP：NAT 后面一整个办公室共享一个出口 IP，
//! 攻击者把某个账号爆破到退避后，**同 IP 的其他人也一起被锁**——
//! 这在交付场景里会直接影响可用性（客户在同一 WiFi 下取片）。
//!
//! 只按用户名：攻击者对同一账号换 IP 即可无限重来，等于没有限制。
//!
//! 两个维度都记、任一超限即拒绝，既挡住单 IP 爆破，也挡住分布式针对单一账号。
//!
//! ## 状态存在哪
//!
//! 进程内存（`HashMap`），**不落库**。理由：
//! - 失败计数是短时状态，落库会带来「数据库被打爆」这个新问题；
//! - 重启后计数清零是可接受的：进程重启本身在公网上是罕见事件，
//!   攻击者要利用这点需要先让服务重启，而那通常需要本地权限。
//!
//! 如果将来要跨重启生效，可换成 Redis 或独立表，但那属于过度设计。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 同一维度连续失败多少次后开始退避。
const THRESHOLD: u32 = 5;
/// 首次退避的时长（秒）。
///
/// 原先是 2^0 = 1 秒，那对人毫无意义：输错 5 次密码只被挡 1 秒，
/// 再点一下失败计数又涨、窗口变 2 秒 —— 用户还没来得及反应就被放行，
/// 而提示里的数字还在原地不动。现在从 5 秒起步，逐次翻倍到 15 分钟封顶。
const BASE_BACKOFF: u64 = 5;
/// 退避的封顶时长。
const MAX_BACKOFF: Duration = Duration::from_secs(900); // 15 分钟
/// 成功登录后多久清空该维度的记录。
const RESET_AFTER: Duration = Duration::from_secs(1800); // 30 分钟
/// 定期清理陈旧条目的间隔，防止 map 无限增长。
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Default, Clone, Copy)]
struct Bucket {
    /// 连续失败次数。
    failures: u32,
    /// 最近一次失败的时刻；None 表示这条记录存在但还没有失败时间
    /// （实际不会发生——只有 record_failure 会建记录——但 Option 让
    /// 「没有时间」这件事有明确表示，而不是靠 Instant 的伪造默认值）。
    last_fail: Option<Instant>,
}

#[derive(Default)]
struct State {
    buckets: HashMap<String, Bucket>,
    last_sweep: Option<Instant>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// 当前应拒绝该请求吗？若拒绝，返回还需要等待的秒数（用于回给客户端）。
///
/// 返回 `None` 表示不限制。
pub fn check(username: &str, ip: &str) -> Option<u64> {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(State::default);

    sweep(state);

    // 两个维度取较严的那个：任一已退避即拒绝。
    // 计数只在 record_success 时清空——**不能在这里顺手清**：
    // 那样每次「未达阈值的检查」都会把失败记录抹掉，退避永远累积不起来。
    let user_wait = backoff_for(state, &username_key(username));
    // IP 维度只在拿到真实 IP 时参与。取不到 IP 的场景（未启用
    // ConnectInfo 的部署、反代后端拿不到来源）如果也按 IP 记账，
    // 所有人会被算进同一个桶——一次爆破就让所有用户一起进退避。
    let ip_wait = if ip.is_empty() {
        None
    } else {
        backoff_for(state, &ip_key(ip))
    };
    match (user_wait, ip_wait) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// 记一次失败。
pub fn record_failure(username: &str, ip: &str) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(State::default);
    let now = Instant::now();

    let keys = if ip.is_empty() {
        vec![username_key(username)]
    } else {
        vec![username_key(username), ip_key(ip)]
    };
    for key in keys {
        let b = state.buckets.entry(key).or_default();
        b.failures = b.failures.saturating_add(1);
        b.last_fail = Some(now);
    }
}

/// 登录成功后清空计数。语义上与 `check` 返回 None 时一致，
/// 单独暴露是为了让调用点读起来是「成功就清」而不是「检查时顺便清」。
pub fn record_success(username: &str, ip: &str) {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.buckets.remove(&username_key(username));
        state.buckets.remove(&ip_key(ip));
    }
}

/// 退避时长 = 2^(失败次数 - 阈值) 秒，封顶 MAX_BACKOFF。
///
/// 用 2 的幂而不是固定线性增长：线性退避下攻击者仍能以恒定速率持续尝试，
/// 指数退避则让单位时间内的尝试次数随时间快速趋近于 0。
fn backoff_for(state: &State, key: &str) -> Option<u64> {
    let b = state.buckets.get(key)?;
    if b.failures < THRESHOLD {
        return None;
    }
    // 距上次失败已久 → 视为冷却已过，交给 sweep 清掉。
    let last = b.last_fail?;
    if last.elapsed() > RESET_AFTER {
        return None;
    }
    // 退避的**基准**时长（与失败次数挂钩，不随时间变化）。
    let exp = b.failures - THRESHOLD;
    let window = BASE_BACKOFF
        .saturating_mul(2u64.saturating_pow(exp.min(20)))
        .min(MAX_BACKOFF.as_secs());

    // ⚠️ 必须减去已过去的时间，返回**剩余**秒数。
    //
    // 原实现直接返回 window，而 window 是「从最后一次失败起算的整个冷却时长」，
    // 不是「还剩多久」。于是前端收到的永远是同一个固定值——
    // 用户看到「请在 1 秒后重试」一直不变，而真实剩余时间在悄悄减少；
    // 更糟的是他若真的重试并再次失败，failures 上涨、window 翻倍，
    // 提示从 1 秒变成 2 秒——越点越长。
    //
    // 窗口已过 → 返回 None 解除限流。
    //
    // ⚠️ 这里必须返回 None，不能用 `.max(1)` 兜底：那样「剩余不足 1 秒」
    // 也会被抬高成 1，于是 check 永远返回 Some，退避**永远不会结束**，
    // 用户被永久锁在门外 —— 那比原来「提示不计时」严重得多。
    //
    // 代价是最后不足 1 秒的窗口里，用户会看到「请在 1 秒后重试」而实际
    // 只需再等几百毫秒。这是可接受的：前端有倒计时，归零后自动放行；
    // 而「永久锁定」是不可接受的。
    let elapsed = last.elapsed().as_secs();
    let remaining = window.saturating_sub(elapsed);
    if remaining == 0 {
        return None;
    }
    Some(remaining)
}

/// 清理超过 RESET_AFTER 未活动的条目。
///
/// 没有这一步的话，攻击者用随机用户名轮番尝试会让 map 无限增长——
/// 限流器本身变成一个内存耗尽入口。
fn sweep(state: &mut State) {
    let now = Instant::now();
    if let Some(last) = state.last_sweep {
        if now.duration_since(last) < SWEEP_INTERVAL {
            return;
        }
    }
    state.last_sweep = Some(now);
    state
        .buckets
        .retain(|_, b| b.last_fail.is_some_and(|t| t.elapsed() <= RESET_AFTER));
}

fn username_key(u: &str) -> String {
    format!("u:{u}")
}

fn ip_key(ip: &str) -> String {
    format!("i:{ip}")
}

/// 仅供测试：清空全部状态，避免用例之间互相污染。
#[cfg(test)]
pub fn reset_for_test() {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// 本模块的限流状态是**进程级全局**（生产代码就该如此），
    /// 而 Rust 测试默认多线程并行。于是 `reset_for_test()` 一执行，
    /// 别的用例正在观察的那个桶就没了 —— 表现为「第一次 5，1.1 秒后 9」
    /// 这种自相矛盾的数字。
    ///
    /// 用一把测试锁把整组串行化即可；生产代码的全局性不受影响。
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    /// 每个用例开头调用，拿到锁期间本组只有一个用例在跑。
    fn setup() -> MutexGuard<'static, ()> {
        let guard = TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        guard
    }

    #[test]
    fn fresh_key_is_not_throttled() {
        let _lock = setup();
        assert_eq!(check("alice", "1.1.1.1"), None);
    }

    #[test]
    fn throttles_after_threshold_failures() {
        let _lock = setup();
        for _ in 0..THRESHOLD {
            record_failure("alice", "1.1.1.1");
        }
        let wait = check("alice", "1.1.1.1").expect("达到阈值后应被限流");
        assert!(wait >= 1, "退避时长应为正数秒，实际 {wait}");
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let _lock = setup();
        let mut prev = 0u64;
        for i in 0..40 {
            record_failure("bob", "2.2.2.2");
            if let Some(w) = check("bob", "2.2.2.2") {
                assert!(w >= prev, "退避应单调不减（第 {i} 次）");
                assert!(w <= MAX_BACKOFF.as_secs(), "不应超过封顶值");
                prev = w;
            }
        }
        assert_eq!(prev, MAX_BACKOFF.as_secs(), "多次失败后应停在封顶值");
    }

    #[test]
    fn ip_dimension_throttles_independently() {
        let _lock = setup();
        // 只打同一个 IP 上的多个不同账号
        for name in ["a", "b", "c", "d", "e", "f"] {
            record_failure(name, "3.3.3.3");
        }
        // 同 IP 换一个从未失败过的用户名，也应被 IP 维度拦住
        assert!(check("brand-new-user", "3.3.3.3").is_some(),
            "IP 维度应独立生效，否则换用户名即可绕过");
    }

    #[test]
    fn username_dimension_throttles_across_ips() {
        let _lock = setup();
        // 换 IP 打同一个账号
        for i in 0..6 {
            record_failure("carol", &format!("10.0.0.{i}"));
        }
        assert!(check("carol", "192.168.1.1").is_some(),
            "账号维度应独立生效，否则换 IP 即可绕过");
    }

    /// 回归：返回的必须是**剩余**秒数，而不是固定窗口。
    ///
    /// 原实现直接返回退避窗口（2^(失败次数-阈值)），于是第 5 次失败后
    /// 永远返回 1 —— 前端弹出「请在 1 秒后重试」后数字再也不变，
    /// 用户以为程序卡住；真去重试还会因为计数上涨而变成 2 秒，越点越长。
    #[test]
    fn backoff_counts_down_as_time_passes() {
        let _lock = setup();
        for _ in 0..THRESHOLD {
            record_failure("carol", "9.9.9.9");
        }
        let first = check("carol", "9.9.9.9").expect("达到阈值后应被限流");

        assert_eq!(first, BASE_BACKOFF, "首窗应从 BASE_BACKOFF 起步");

        std::thread::sleep(Duration::from_millis(1100));
        let later = check("carol", "9.9.9.9").expect("仍在退避窗口内");

        assert!(
            later < first,
            "剩余秒数应随时间减少：第一次 {first}，1.1 秒后 {later}"
        );
    }

    /// 剩余时间耗尽后必须归零（0 表示不再限流）。
    #[test]
    fn backoff_expires_and_stops_throttling() {
        let _lock = setup();
        for _ in 0..THRESHOLD {
            record_failure("dave", "8.8.8.8");
        }
        assert!(check("dave", "8.8.8.8").is_some());
        // 首窗是 BASE_BACKOFF 秒，睡过它就该恢复
        std::thread::sleep(Duration::from_millis(BASE_BACKOFF * 1000 + 200));
        assert_eq!(
            check("dave", "8.8.8.8"),
            None,
            "退避时间耗尽后必须解除限流，否则用户会被永久锁在门外"
        );
    }

    /// 仍被限流时，返回的秒数必须落在 (0, 窗口] 区间内。
    ///
    /// 这是两条相反要求的交汇点：
    ///   · 不能返回 0（提示「请在 0 秒后重试」自相矛盾）
    ///   · 也不能为了「至少 1 秒」而把窗口耗尽后的状态也抬高 —— 那会导致
    ///     限流永不结束、用户被永久锁在门外。
    #[test]
    fn remaining_stays_within_window_while_throttled() {
        let _lock = setup();
        for _ in 0..THRESHOLD {
            record_failure("erin", "7.7.7.7");
        }
        let w = check("erin", "7.7.7.7").expect("应被限流");
        assert!(w >= 1, "仍被限流时剩余秒数至少为 1，实际 {w}");
        assert!(
            w <= BASE_BACKOFF,
            "剩余秒数不应超过当前窗口 {BASE_BACKOFF}，实际 {w}"
        );
    }

    #[test]
    fn success_resets_counters() {
        let _lock = setup();
        for _ in 0..8 {
            record_failure("dave", "4.4.4.4");
        }
        assert!(check("dave", "4.4.4.4").is_some());
        record_success("dave", "4.4.4.4");
        assert_eq!(check("dave", "4.4.4.4"), None, "成功后应解除限流");
    }

    #[test]
    fn unrelated_identity_is_untouched() {
        let _lock = setup();
        for _ in 0..10 {
            record_failure("eve", "5.5.5.5");
        }
        assert!(check("eve", "5.5.5.5").is_some());
        assert_eq!(check("frank", "6.6.6.6"), None, "他人不应受影响");
    }
}
