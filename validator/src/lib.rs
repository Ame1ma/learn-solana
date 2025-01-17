#![allow(clippy::arithmetic_side_effects)]
pub use solana_test_validator as test_validator;
use {
    console::style,
    fd_lock::{RwLock, RwLockWriteGuard},
    indicatif::{ProgressDrawTarget, ProgressStyle},
    std::{
        borrow::Cow,
        env,
        fmt::Display,
        fs::{File, OpenOptions},
        path::Path,
        process::exit,
        thread::JoinHandle,
        time::Duration,
    },
};

pub mod admin_rpc_service;
pub mod bootstrap;
pub mod cli;
pub mod dashboard;

/// 重定向错误到文件
#[cfg(unix)]
fn redirect_stderr(filename: &str) {
    use std::os::unix::io::AsRawFd;
    match OpenOptions::new().create(true).append(true).open(filename) {
        Ok(file) => unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
        },
        Err(err) => eprintln!("Unable to open {filename}: {err}"),
    }
}

// Redirect stderr to a file with support for logrotate by sending a SIGUSR1 to the process.
//
// Upon success, future `log` macros and `eprintln!()` can be found in the specified log file.
/// 通过向进程发送SIGUSR1，将stderr重定向到支持logrotate的文件。
/// SIGUSR1 是一种 用户定义的信号，它属于 UNIX 和 Linux 系统中的信号机制。它的全称是 Signal User 1，通常用于用户或应用程序之间的自定义通信。
/// 不同的程序或系统进程可以根据自己的需求定义如何处理这个信号。SIGUSR1 是一个 信号，其作用取决于接受信号的程序如何进行处理。
pub fn redirect_stderr_to_file(logfile: Option<String>) -> Option<JoinHandle<()>> {
    // Default to RUST_BACKTRACE=1 for more informative validator logs
    /// 没设置 RUST_BACKTRACE 的话给它开启
    if env::var_os("RUST_BACKTRACE").is_none() {
        env::set_var("RUST_BACKTRACE", "1")
    }

    match logfile {
        /// 没有指定路径
        None => {
            /// 初始化日志器
            solana_logger::setup_with_default_filter();
            None
        }
        Some(logfile) => {
            #[cfg(unix)]
            {
                use log::info;
                /// 监听 SIGUSR1
                let mut signals =
                    signal_hook::iterator::Signals::new([signal_hook::consts::SIGUSR1])
                        .unwrap_or_else(|err| {
                            eprintln!("Unable to register SIGUSR1 handler: {err:?}");
                            exit(1);
                        });

                solana_logger::setup_with_default_filter();
                redirect_stderr(&logfile);
                Some(
                    std::thread::Builder::new()
                        .name("solSigUsr1".into())
                        .spawn(move || {
                            /// 一旦接收到 SIGUSR1 信号，线程会进入 for 循环中的处理代码。
                            /// 这段代码的效果是：程序运行期间，任何时候接收到 SIGUSR1 信号，都会触发日志文件的重新打开。
                            /// 具体来说，程序会重新设置 stderr 的输出位置，将日志输出到指定的 logfile 中。
                            /// 这种处理方式通常用于 日志轮换 或者 日志刷新
                            /// 当日志文件变得过大时，程序可能希望重新打开一个新的日志文件。
                            for signal in signals.forever() {
                                info!(
                                    "received SIGUSR1 ({}), reopening log file: {:?}",
                                    signal, logfile
                                );
                                redirect_stderr(&logfile);
                            }
                        })
                        .unwrap(),
                )
            }
            #[cfg(not(unix))]
            {
                println!("logrotate is not supported on this platform");
                solana_logger::setup_file_with_default(&logfile, solana_logger::DEFAULT_FILTER);
                None
            }
        }
    }
}

/// 优美打印 名字 值，名字用粗体
pub fn format_name_value(name: &str, value: &str) -> String {
    format!("{} {}", style(name).bold(), value)
}
/// Pretty print a "name value"
/// 优美打印 名字 值，名字用粗体
pub fn println_name_value(name: &str, value: &str) {
    println!("{}", format_name_value(name, value));
}

/// Creates a new process bar for processing that will take an unknown amount of time
/// 新建进度条
pub fn new_spinner_progress_bar() -> ProgressBar {
    /// 进度条库 indicatif
    let progress_bar = indicatif::ProgressBar::new(42);
    /// 设置绘制目标，绘制到标准输出
    progress_bar.set_draw_target(ProgressDrawTarget::stdout());
    /// 设置样式
    progress_bar.set_style(
        ProgressStyle::default_spinner()
            /// 绿色进度条，详细信息
            .template("{spinner:.green} {wide_msg}")
            .expect("ProgresStyle::template direct input to be correct"),
    );
    /// 100 毫秒刷新一次进度条
    progress_bar.enable_steady_tick(Duration::from_millis(100));

    ProgressBar {
        progress_bar,
        is_term: console::Term::stdout().is_term(),
    }
}

pub struct ProgressBar {
    progress_bar: indicatif::ProgressBar,
    is_term: bool,
}

impl ProgressBar {
    /// 设置进度条显示信息
    pub fn set_message<T: Into<Cow<'static, str>> + Display>(&self, msg: T) {
        if self.is_term {
            self.progress_bar.set_message(msg);
        } else {
            /// 不是终端的话直接打印
            println!("{msg}");
        }
    }

    pub fn println<I: AsRef<str>>(&self, msg: I) {
        self.progress_bar.println(msg);
    }

    pub fn abandon_with_message<T: Into<Cow<'static, str>> + Display>(&self, msg: T) {
        if self.is_term {
            self.progress_bar.abandon_with_message(msg);
        } else {
            println!("{msg}");
        }
    }
}

pub fn ledger_lockfile(ledger_path: &Path) -> RwLock<File> {
    let lockfile = ledger_path.join("ledger.lock");
    fd_lock::RwLock::new(
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(lockfile)
            .unwrap(),
    )
}

pub fn lock_ledger<'lock>(
    ledger_path: &Path,
    ledger_lockfile: &'lock mut RwLock<File>,
) -> RwLockWriteGuard<'lock, File> {
    ledger_lockfile.try_write().unwrap_or_else(|_| {
        println!(
            "Error: Unable to lock {} directory. Check if another validator is running",
            ledger_path.display()
        );
        exit(1);
    })
}
