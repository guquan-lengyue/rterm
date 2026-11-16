//! TransferQueue — 上传 / 下载队列（2 并发 worker，进度、暂停、取消、重试）。

use crate::service::SftpService;
use futures::channel::mpsc::UnboundedSender;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// 获取 Mutex 守卫；锁中毒（工作线程 panic）时记录错误并恢复内部数据。
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::error!("[sftp] Mutex poisoned，恢复内部状态继续运行");
            poisoned.into_inner()
        }
    }
}

/// 每次读写的块大小。
pub const CHUNK_SIZE: usize = 64 * 1024;
/// 单任务最多尝试次数（含首次）。
pub const MAX_ATTEMPTS: u8 = 3;
/// 进度事件最小间隔。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

impl TransferDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Upload => "上传",
            Self::Download => "下载",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TransferState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队中",
            Self::Running => "传输中",
            Self::Paused => "已暂停",
            Self::Completed => "已完成",
            Self::Failed => "失败",
            Self::Cancelled => "已取消",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }

    fn code(self) -> u8 {
        match self {
            Self::Queued => 0,
            Self::Running => 1,
            Self::Paused => 2,
            Self::Completed => 3,
            Self::Failed => 4,
            Self::Cancelled => 5,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Queued,
            1 => Self::Running,
            2 => Self::Paused,
            3 => Self::Completed,
            4 => Self::Failed,
            _ => Self::Cancelled,
        }
    }
}

/// 队列状态的只读快照（供 UI 渲染）。
#[derive(Clone, Debug)]
pub struct TransferSnapshot {
    pub id: Uuid,
    pub direction: TransferDirection,
    pub remote_path: String,
    pub local_path: PathBuf,
    pub total: u64,
    pub transferred: u64,
    pub state: TransferState,
    pub error: Option<String>,
}

impl TransferSnapshot {
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            if self.state == TransferState::Completed {
                1.0
            } else {
                0.0
            }
        } else {
            (self.transferred as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
}

/// 传输事件（UI 层转成 AppEvent 广播）。
#[derive(Clone, Debug)]
pub enum TransferEvent {
    Started {
        id: Uuid,
        direction: TransferDirection,
        remote_path: String,
        local_path: PathBuf,
    },
    Progress {
        id: Uuid,
        transferred: u64,
        total: u64,
    },
    Paused {
        id: Uuid,
    },
    Resumed {
        id: Uuid,
    },
    Completed {
        id: Uuid,
    },
    Failed {
        id: Uuid,
        reason: String,
    },
    Cancelled {
        id: Uuid,
    },
}

struct TransferTask {
    id: Uuid,
    direction: TransferDirection,
    remote_path: String,
    local_path: PathBuf,
    total: u64,
    transferred: AtomicU64,
    state: AtomicU8,
    pause: AtomicBool,
    cancel: AtomicBool,
    error: Mutex<Option<String>>,
    started_emitted: AtomicBool,
}

impl TransferTask {
    fn new(
        direction: TransferDirection,
        remote_path: String,
        local_path: PathBuf,
        total: u64,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            direction,
            remote_path,
            local_path,
            total,
            transferred: AtomicU64::new(0),
            state: AtomicU8::new(TransferState::Queued.code()),
            pause: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            error: Mutex::new(None),
            started_emitted: AtomicBool::new(false),
        }
    }

    fn state(&self) -> TransferState {
        TransferState::from_code(self.state.load(Ordering::SeqCst))
    }

    fn set_state(&self, state: TransferState) {
        self.state.store(state.code(), Ordering::SeqCst);
    }

    fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            id: self.id,
            direction: self.direction,
            remote_path: self.remote_path.clone(),
            local_path: self.local_path.clone(),
            total: self.total,
            transferred: self.transferred.load(Ordering::Relaxed),
            state: self.state(),
            error: lock_or_recover(&self.error).clone(),
        }
    }
}

enum Command {
    Run(Arc<TransferTask>),
    Shutdown,
}

/// 上传 / 下载队列。
pub struct TransferQueue {
    tasks: Arc<Mutex<Vec<Arc<TransferTask>>>>,
    /// 每个 worker 一条独立通道，避免共享 receiver 导致的串行化。
    worker_txs: Vec<mpsc::Sender<Command>>,
    /// 轮询选择 worker 的计数器。
    next_worker: AtomicUsize,
    events: UnboundedSender<TransferEvent>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl TransferQueue {
    /// 启动 `concurrency` 个 worker；每个 worker 顺序处理自己通道上的任务。
    pub fn new(
        service: Arc<SftpService>,
        events: UnboundedSender<TransferEvent>,
        concurrency: usize,
    ) -> Self {
        let concurrency = concurrency.max(1);
        let tasks = Arc::new(Mutex::new(Vec::new()));
        let mut worker_txs = Vec::with_capacity(concurrency);
        let mut workers = Vec::with_capacity(concurrency);

        for index in 0..concurrency {
            let service = service.clone();
            let events = events.clone();
            let (tx, rx) = mpsc::channel::<Command>();
            let handle = thread::Builder::new()
                .name(format!("rterm-sftp-transfer-{index}"))
                .spawn(move || worker_loop(service, rx, events))
                .expect("无法启动 SFTP 传输线程");
            worker_txs.push(tx);
            workers.push(handle);
        }

        Self {
            tasks,
            worker_txs,
            next_worker: AtomicUsize::new(0),
            events,
            workers: Mutex::new(workers),
        }
    }

    /// 入队一个上传任务；`total` 由调用方通过本地 metadata 提供。
    pub fn enqueue_upload(&self, remote_path: &str, local_path: PathBuf) -> std::io::Result<Uuid> {
        let total = std::fs::metadata(&local_path)?.len();
        Ok(self.enqueue(
            TransferDirection::Upload,
            remote_path.to_string(),
            local_path,
            total,
        ))
    }

    /// 入队一个下载任务；`total` 由调用方通过远端 stat 提供。
    pub fn enqueue_download(&self, remote_path: String, local_path: PathBuf, total: u64) -> Uuid {
        self.enqueue(TransferDirection::Download, remote_path, local_path, total)
    }

    fn enqueue(
        &self,
        direction: TransferDirection,
        remote_path: String,
        local_path: PathBuf,
        total: u64,
    ) -> Uuid {
        let task = Arc::new(TransferTask::new(direction, remote_path, local_path, total));
        let id = task.id;
        lock_or_recover(&self.tasks).push(task.clone());
        // 轮询分发到各 worker，使多个任务能真正并行。
        let index = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.worker_txs.len();
        let _ = self.worker_txs[index].send(Command::Run(task));
        id
    }

    /// 当前队列快照（含历史任务）。
    pub fn snapshots(&self) -> Vec<TransferSnapshot> {
        lock_or_recover(&self.tasks)
            .iter()
            .map(|task| task.snapshot())
            .collect()
    }

    pub fn pause(&self, id: Uuid) {
        if let Some(task) = self.find(id) {
            if task.state().is_active() {
                task.pause.store(true, Ordering::SeqCst);
                if task.state() == TransferState::Running {
                    task.set_state(TransferState::Paused);
                    let _ = self.events.unbounded_send(TransferEvent::Paused { id });
                }
            }
        }
    }

    pub fn resume(&self, id: Uuid) {
        if let Some(task) = self.find(id) {
            if task.pause.swap(false, Ordering::SeqCst) {
                task.set_state(TransferState::Running);
                let _ = self.events.unbounded_send(TransferEvent::Resumed { id });
            }
        }
    }

    pub fn cancel(&self, id: Uuid) {
        if let Some(task) = self.find(id) {
            if task.state().is_active() {
                task.cancel.store(true, Ordering::SeqCst);
            }
        }
    }

    /// 关闭队列：取消所有任务、通知 worker 退出并等待线程结束。
    pub fn shutdown(&self) {
        {
            let tasks = lock_or_recover(&self.tasks);
            for task in tasks.iter() {
                task.cancel.store(true, Ordering::SeqCst);
            }
        }
        for tx in &self.worker_txs {
            let _ = tx.send(Command::Shutdown);
        }
        // 不在调用线程（UI）里 join：后台线程可能正卡在 libssh2 网络读写，
        // 同步 join 会冻结界面。交给一个短生命周期的回收线程等待退出。
        let handles = {
            let mut workers = lock_or_recover(&self.workers);
            std::mem::take(&mut *workers)
        };
        if !handles.is_empty() {
            let _ = std::thread::Builder::new()
                .name("rterm-sftp-reaper".into())
                .spawn(move || {
                    for handle in handles {
                        if let Err(error) = handle.join() {
                            log::warn!("[sftp] transfer worker join failed: {error:?}");
                        }
                    }
                });
        }
    }

    fn find(&self, id: Uuid) -> Option<Arc<TransferTask>> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|task| task.id == id)
            .cloned()
    }
}

fn worker_loop(
    service: Arc<SftpService>,
    rx: mpsc::Receiver<Command>,
    events: UnboundedSender<TransferEvent>,
) {
    loop {
        let Ok(command) = rx.recv() else {
            return;
        };
        match command {
            Command::Shutdown => return,
            Command::Run(task) => {
                run_task(&service, &task, &events);
            }
        }
    }
}

fn run_task(
    service: &SftpService,
    task: &Arc<TransferTask>,
    events: &UnboundedSender<TransferEvent>,
) {
    if task.cancel.load(Ordering::SeqCst) {
        task.set_state(TransferState::Cancelled);
        let _ = events.unbounded_send(TransferEvent::Cancelled { id: task.id });
        cleanup_partial(service, task);
        return;
    }

    if !task.started_emitted.swap(true, Ordering::SeqCst) {
        let _ = events.unbounded_send(TransferEvent::Started {
            id: task.id,
            direction: task.direction,
            remote_path: task.remote_path.clone(),
            local_path: task.local_path.clone(),
        });
    }
    task.set_state(TransferState::Running);

    let mut attempt = 0;
    loop {
        attempt += 1;
        task.transferred.store(0, Ordering::Relaxed);
        let result = transfer_once(service, task, events);
        match result {
            Ok(Outcome::Completed) => {
                task.set_state(TransferState::Completed);
                let _ = events.unbounded_send(TransferEvent::Completed { id: task.id });
                return;
            }
            Ok(Outcome::Cancelled) => {
                task.set_state(TransferState::Cancelled);
                let _ = events.unbounded_send(TransferEvent::Cancelled { id: task.id });
                cleanup_partial(service, task);
                return;
            }
            Err(err) => {
                log::warn!(
                    "[sftp] transfer {} attempt {attempt}/{MAX_ATTEMPTS} failed: {err}",
                    task.id
                );
                if attempt >= MAX_ATTEMPTS {
                    let reason = err.to_string();
                    *lock_or_recover(&task.error) = Some(reason.clone());
                    task.set_state(TransferState::Failed);
                    // 失败时只清理临时文件，不动目标路径上的已有文件。
                    cleanup_partial(service, task);
                    let _ = events.unbounded_send(TransferEvent::Failed {
                        id: task.id,
                        reason,
                    });
                    return;
                }
                thread::sleep(Duration::from_millis(200 * attempt as u64));
            }
        }
    }
}

enum Outcome {
    Completed,
    Cancelled,
}

fn transfer_once(
    service: &SftpService,
    task: &Arc<TransferTask>,
    events: &UnboundedSender<TransferEvent>,
) -> anyhow::Result<Outcome> {
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut last_emit = Instant::now();
    let mut paused_announced = false;

    let mut wait_if_paused = |task: &Arc<TransferTask>| -> bool {
        while task.pause.load(Ordering::SeqCst) {
            if task.cancel.load(Ordering::SeqCst) {
                return false;
            }
            if !paused_announced {
                paused_announced = true;
                task.set_state(TransferState::Paused);
                let _ = events.unbounded_send(TransferEvent::Paused { id: task.id });
            }
            thread::sleep(Duration::from_millis(50));
        }
        if paused_announced {
            paused_announced = false;
            task.set_state(TransferState::Running);
            let _ = events.unbounded_send(TransferEvent::Resumed { id: task.id });
        }
        !task.cancel.load(Ordering::SeqCst)
    };

    match task.direction {
        TransferDirection::Upload => {
            let target = PathBuf::from(&task.remote_path);
            let part = upload_part_path(&task.remote_path, task.id);
            let mut local = File::open(&task.local_path)?;
            // 每次尝试都从临时文件重新开始（create 会截断上次残留的临时文件）。
            let mut remote = service.sftp().create(Path::new(&part))?;
            loop {
                if !wait_if_paused(task) {
                    return Ok(Outcome::Cancelled);
                }
                let read = local.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                remote.write_all(&buffer[..read])?;
                let transferred =
                    task.transferred.fetch_add(read as u64, Ordering::Relaxed) + read as u64;
                maybe_emit_progress(task, events, transferred, &mut last_emit);
            }
            remote.flush()?;
            drop(remote);
            // 完整写入临时文件后才改名到目标路径。
            finalize_upload(service, Path::new(&part), &target)?;
            let _ = events.unbounded_send(TransferEvent::Progress {
                id: task.id,
                transferred: task.transferred.load(Ordering::Relaxed),
                total: task.total,
            });
            Ok(Outcome::Completed)
        }
        TransferDirection::Download => {
            let target = task.local_path.clone();
            let part = download_part_path(&target, task.id);
            let mut remote = service.sftp().open(Path::new(&task.remote_path))?;
            if let Some(parent) = part.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut local = File::create(&part)?;
            loop {
                if !wait_if_paused(task) {
                    return Ok(Outcome::Cancelled);
                }
                let read = remote.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                local.write_all(&buffer[..read])?;
                let transferred =
                    task.transferred.fetch_add(read as u64, Ordering::Relaxed) + read as u64;
                maybe_emit_progress(task, events, transferred, &mut last_emit);
            }
            local.flush()?;
            drop(local);
            // 完整写入临时文件后才改名到目标路径。
            finalize_download(&part, &target)?;
            let _ = events.unbounded_send(TransferEvent::Progress {
                id: task.id,
                transferred: task.transferred.load(Ordering::Relaxed),
                total: task.total,
            });
            Ok(Outcome::Completed)
        }
    }
}

fn maybe_emit_progress(
    task: &Arc<TransferTask>,
    events: &UnboundedSender<TransferEvent>,
    transferred: u64,
    last_emit: &mut Instant,
) {
    if last_emit.elapsed() >= PROGRESS_INTERVAL {
        *last_emit = Instant::now();
        let _ = events.unbounded_send(TransferEvent::Progress {
            id: task.id,
            transferred,
            total: task.total,
        });
    }
}

/// 下载临时文件路径：`${target}.rterm-part-${task_id}`。
pub fn download_part_path(target: &Path, id: Uuid) -> PathBuf {
    let mut path = target.as_os_str().to_os_string();
    path.push(format!(".rterm-part-{id}"));
    PathBuf::from(path)
}

/// 上传临时文件路径：`${target}.rterm-part-${task_id}`。
pub fn upload_part_path(target: &str, id: Uuid) -> String {
    format!("{target}.rterm-part-{id}")
}

/// 下载完成后把临时文件改名为目标文件。
///
/// 只在完整下载成功后调用；失败时不触碰目标路径上已有的文件。
/// Windows 的 rename 不覆盖已有目标，因此在完整下载成功后才删除旧目标再重试。
fn finalize_download(part: &Path, target: &Path) -> std::io::Result<()> {
    match std::fs::rename(part, target) {
        Ok(()) => Ok(()),
        Err(first) => {
            if target.exists() {
                std::fs::remove_file(target)?;
                std::fs::rename(part, target)
            } else {
                Err(first)
            }
        }
    }
}

/// 上传完成后把远端临时文件改名为目标文件。
///
/// 若目标已存在导致 rename 失败，在完整传输成功后 unlink 目标再重试一次。
fn finalize_upload(service: &SftpService, part: &Path, target: &Path) -> anyhow::Result<()> {
    match service.sftp().rename(part, target, None) {
        Ok(()) => Ok(()),
        Err(first) => {
            if service.sftp().stat(target).is_ok() {
                service
                    .sftp()
                    .unlink(target)
                    .map_err(|error| anyhow::anyhow!("覆盖远端目标失败: {error}"))?;
                service
                    .sftp()
                    .rename(part, target, None)
                    .map_err(|error| anyhow::anyhow!("重命名失败: {error}"))?;
                Ok(())
            } else {
                Err(anyhow::anyhow!("重命名失败: {first}"))
            }
        }
    }
}

/// 只清理临时文件，绝不触碰目标路径上可能已存在的文件。
fn cleanup_partial(service: &SftpService, task: &Arc<TransferTask>) {
    match task.direction {
        TransferDirection::Download => {
            let part = download_part_path(&task.local_path, task.id);
            let _ = std::fs::remove_file(&part);
        }
        TransferDirection::Upload => {
            let part = upload_part_path(&task.remote_path, task.id);
            let _ = service.sftp().unlink(Path::new(&part));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_codes_roundtrip() {
        for state in [
            TransferState::Queued,
            TransferState::Running,
            TransferState::Paused,
            TransferState::Completed,
            TransferState::Failed,
            TransferState::Cancelled,
        ] {
            assert_eq!(TransferState::from_code(state.code()), state);
        }
    }

    #[test]
    fn active_states_are_queued_running_paused() {
        assert!(TransferState::Queued.is_active());
        assert!(TransferState::Running.is_active());
        assert!(TransferState::Paused.is_active());
        assert!(!TransferState::Completed.is_active());
        assert!(!TransferState::Failed.is_active());
        assert!(!TransferState::Cancelled.is_active());
    }

    #[test]
    fn progress_is_clamped() {
        let base = TransferSnapshot {
            id: Uuid::nil(),
            direction: TransferDirection::Upload,
            remote_path: String::new(),
            local_path: PathBuf::new(),
            total: 100,
            transferred: 50,
            state: TransferState::Running,
            error: None,
        };
        assert!((base.progress() - 0.5).abs() < f32::EPSILON);
        let done = TransferSnapshot {
            transferred: 200,
            ..base.clone()
        };
        assert_eq!(done.progress(), 1.0);
        let empty_done = TransferSnapshot {
            total: 0,
            transferred: 0,
            state: TransferState::Completed,
            ..base
        };
        assert_eq!(empty_done.progress(), 1.0);
    }

    #[test]
    fn direction_labels() {
        assert_eq!(TransferDirection::Upload.label(), "上传");
        assert_eq!(TransferDirection::Download.label(), "下载");
    }

    #[test]
    fn download_part_path_appends_suffix() {
        let id = Uuid::nil();
        let part = download_part_path(Path::new("C:\\Users\\me\\a.txt"), id);
        assert_eq!(
            part.to_string_lossy(),
            format!("C:\\Users\\me\\a.txt.rterm-part-{id}")
        );
    }

    #[test]
    fn upload_part_path_appends_suffix() {
        let id = Uuid::nil();
        assert_eq!(
            upload_part_path("/home/me/a.txt", id),
            format!("/home/me/a.txt.rterm-part-{id}")
        );
    }
}
