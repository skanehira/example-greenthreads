#![feature(naked_functions)]
use std::arch::asm;

const DEFAULT_STACK_SIZE: usize = 1024 * 1024 * 2;
const MAX_THREADS: usize = 4;
static mut RUNTIME: usize = 0;

pub struct Runtime {
    threads: Vec<Thread>,
    current: usize,
}

#[derive(PartialEq, Eq, Debug)]
enum State {
    Available, // 利用可能
    Running,   // 実行中
    Ready,     // 再開可能
}

struct Thread {
    id: usize,
    stack: Vec<u8>,
    ctx: ThreadContext,
    state: State,
}

impl Thread {
    fn new(id: usize) -> Self {
        Thread {
            id,
            stack: vec![0_u8; DEFAULT_STACK_SIZE],
            ctx: ThreadContext::default(),
            state: State::Available,
        }
    }
}

#[derive(Debug, Default)]
#[repr(C)]
struct ThreadContext {
    rsp: u64,
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbx: u64,
    rbp: u64,
}

impl Runtime {
    pub fn new() -> Self {
        let base_thread = Thread {
            id: 0,
            stack: vec![0_u8; DEFAULT_STACK_SIZE],
            ctx: ThreadContext::default(),
            state: State::Running,
        };

        let mut threads = vec![base_thread];
        let mut available_threads: Vec<Thread> = (1..MAX_THREADS).map(Thread::new).collect();
        threads.append(&mut available_threads);

        Runtime {
            threads,
            current: 0,
        }
    }

    pub fn init(&self) {
        unsafe {
            let r_ptr: *const Runtime = self;
            RUNTIME = r_ptr as usize;
        }
    }

    pub fn run(&mut self) -> ! {
        while self.t_yield() {}
        std::process::exit(0);
    }

    fn t_return(&mut self) {
        if self.current != 0 {
            // タスクの処理が終わったときにこの関数が呼ばれるため、現在のスレッドを
            // Ready(再開可能)ではなくAvailable(利用可能)の状態にする
            self.threads[self.current].state = State::Available;
            self.t_yield();
        }
    }

    fn t_yield(&mut self) -> bool {
        let mut pos = self.current;
        // 再開可能なスレッドを探す
        // 再開可能なスレッドがない場合は処理しない
        while self.threads[pos].state != State::Ready {
            pos += 1;
            if pos == self.threads.len() {
                pos = 0;
            }

            if pos == self.current {
                return false;
            }
        }

        // 現在のスレッドの状態をReady(再開可能)に変更
        // NOTE: 現在のスレッドがすでに利用可能の場合は状態を変えない
        if self.threads[self.current].state != State::Available {
            self.threads[self.current].state = State::Ready;
        }

        // 再開可能なスレッドの状態をRunning(実行中)に変更
        self.threads[pos].state = State::Running;
        let old_pos = self.current;
        // 実行中のスレッドを切り替え先のスレッドに変更
        self.current = pos;

        unsafe {
            // 現在スレッドの再開処理に必要なコンテキスト情報を取得
            let old: *mut ThreadContext = &mut self.threads[old_pos].ctx;
            // 再開するスレッドの再開処理に必要なコンテキスト情報を取得
            let new: *const ThreadContext = &self.threads[pos].ctx;
            // それぞれのコンテキスト情報のアドレスをレジスタに保持
            // NOTE: clobber_abi("C"): レジスタにあるデータをswitchする前に、スタックにプッシュし、関数が戻ってきたらレジスタに戻すってことらしい
            asm!("call switch", in("rdi") old, in("rsi") new, clobber_abi("C"));
        }

        // コンパイラの最適化をさせないようにするためらしい(よくわからん)
        self.threads.len() > 0
    }

    pub fn spawn(&mut self, f: fn()) {
        // 再開可能なスレッドを取得
        // 見つからない場合はpanicする
        let available = self
            .threads
            .iter_mut()
            .find(|t| t.state == State::Available)
            .expect("not available thread.");

        let size = available.stack.len();

        unsafe {
            // スタックポインタ
            let s_ptr = available.stack.as_mut_ptr().offset(size as isize);
            // 16byteアライメント
            let s_ptr = (s_ptr as usize & !15) as *mut u8;

            // guard: タスクの処理が完了し、関数が戻ったときに呼ばれる
            std::ptr::write(s_ptr.offset(-16) as *mut u64, guard as u64);
            // skip: 次の命令を実行する、つまりguard関数を実行する
            std::ptr::write(s_ptr.offset(-24) as *mut u64, skip as u64);
            // タスク関数のアドレスを書き込む
            std::ptr::write(s_ptr.offset(-32) as *mut u64, f as u64);
            // タスク関数を実行できるように、スタックポインタのアドレスをrspに書き込む
            available.ctx.rsp = s_ptr.offset(-32) as u64;
        }

        // 現在のスレッドを再開可能の状態に変更
        available.state = State::Ready;
    }
}

fn guard() {
    unsafe {
        let rt_ptr = RUNTIME as *mut Runtime;
        (*rt_ptr).t_return();
    }
}

#[naked]
unsafe extern "C" fn skip() {
    asm!("ret", options(noreturn))
}

pub fn yield_thread() {
    unsafe {
        let rt_ptr = RUNTIME as *mut Runtime;
        (*rt_ptr).t_yield();
    }
}

// 現在のスレッドのスタックをrdiレジスタ退避し、
// 新しいスレッドのスタックをrsiレジスタから取得して上書きする
// NOTE:
//  ThreadContextのフィールドは各8byte(u64)ずつになっているので、offsetも8byteずつ足していく
#[naked]
#[no_mangle]
unsafe extern "C" fn switch() {
    asm!(
        "mov [rdi + 0x00], rsp",
        "mov [rdi + 0x08], r15",
        "mov [rdi + 0x10], r14",
        "mov [rdi + 0x18], r13",
        "mov [rdi + 0x20], r12",
        "mov [rdi + 0x28], rbx",
        "mov [rdi + 0x30], rbp",
        "mov rsp, [rsi + 0x00]",
        "mov r15, [rsi + 0x08]",
        "mov r14, [rsi + 0x10]",
        "mov r13, [rsi + 0x18]",
        "mov r12, [rsi + 0x20]",
        "mov rbx, [rsi + 0x28]",
        "mov rbp, [rsi + 0x30]",
        "ret", options(noreturn)
    );
}
fn main() {
    let mut runtime = Runtime::new();
    runtime.init();
    runtime.spawn(|| {
        println!("THREAD 1 STARTING");
        let id = 1;
        for i in 0..10 {
            println!("thread: {} counter: {}", id, i);
            // スレッド切り替え
            yield_thread();
        }

        println!("THREAD 1 FINISHED");
    });
    runtime.spawn(|| {
        println!("THREAD 2 STARTING");
        let id = 2;
        for i in 0..15 {
            println!("thread: {} counter: {}", id, i);
            // スレッド切り替え
            yield_thread();
        }

        println!("THREAD 2 FINISHED");
    });

    runtime.run();
}
