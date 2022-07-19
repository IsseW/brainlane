#![feature(let_else)]
use std::{
    env, fs,
    sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering},
};

use array_init::array_init;
use console::Term;

enum IInstr {
    /// <
    Last,
    /// >
    Next,
    /// +, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, where 0 = 10
    AddConst(u8),
    /// -
    SubConst(u8),
    /// .
    Print,
    /// ,
    Read,
    /// [
    StartLoop(usize),
    /// ]
    EndLoop(usize),
    /// |
    Wait(usize, u32),
    /// A..=Z
    Send(usize, usize),
    /// a..=z
    Recieve(usize, usize, u32),
}

struct PInstr {
    y: usize,
    x: usize,
    kind: IInstr,
    c: char,
}

impl PInstr {
    fn error(&self, kind: ErrorKind) -> Error {
        Error {
            kind,
            c: self.c,
            x: self.x,
            y: self.y,
        }
    }
}

enum Instr<'a> {
    /// <
    Last,
    /// >
    Next,
    /// +, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0, where 0 = 10
    AddConst(u8),
    /// -
    SubConst(u8),
    /// .
    Print,
    /// ,
    Read,
    /// [
    StartLoop(usize),
    /// ]
    EndLoop(usize),
    /// |
    Wait(&'a Stop, u32),
    /// A..=Z
    Send(&'a Message),
    /// a..=z
    Recieve(&'a Message, u32),
}

#[derive(Default)]
struct Lane<'a> {
    instructions: Vec<Instr<'a>>,
}

impl<'a> Lane<'a> {
    fn run(&self) {
        let mut ptr: usize = 0;
        const MEM: usize = 30000;
        let mut mem = Box::new([0u8; MEM]);

        let mut cptr: usize = 0;

        while cptr < self.instructions.len() {
            let mut new_line = None;

            match &self.instructions[cptr] {
                Instr::AddConst(num) => mem[ptr] = mem[ptr].wrapping_add(*num),
                Instr::SubConst(num) => mem[ptr] = mem[ptr].wrapping_sub(*num),
                Instr::Print => print!("{}", mem[ptr] as char),
                Instr::Read => mem[ptr] = Term::stdout().read_char().unwrap() as u8,
                Instr::Wait(stop, channel) => {
                    stop.wait(*channel);
                }
                Instr::Last => {
                    ptr = (MEM - 1).min(ptr.wrapping_sub(1));
                }
                Instr::Next => {
                    ptr = (ptr + 1) % MEM;
                }
                Instr::StartLoop(end) => {
                    if mem[ptr] == 0 {
                        new_line = Some(*end);
                    }
                }
                Instr::EndLoop(start) => {
                    if mem[ptr] > 0 {
                        new_line = Some(*start);
                    }
                }
                Instr::Send(msg) => {
                    msg.send(mem[ptr]);
                }
                Instr::Recieve(msg, channel) => {
                    mem[ptr] = msg.recieve(*channel);
                }
            }

            cptr = new_line.unwrap_or(cptr + 1);
        }
    }
}

#[derive(Default)]
struct Stop {
    num: u32,
    behind: AtomicU32,
    in_front: AtomicU32,
    lock: AtomicU64,
}

impl Stop {
    fn wait(&self, channel: u32) {
        // Wait if there are is a lock already going on.
        while self.lock.load(Ordering::Relaxed) & (1 << channel) == 1 {}
        // Add one
        self.behind.fetch_add(1, Ordering::Relaxed);

        while self.behind.load(Ordering::Relaxed) < self.num {}

        if self.in_front.fetch_add(1, Ordering::Relaxed) == self.num {
            // This is the last in the lock, reset everything.
            self.behind.store(0, Ordering::Relaxed);
            self.in_front.store(0, Ordering::Relaxed);
            self.lock.store(0, Ordering::Relaxed);
        } else {
            self.lock.fetch_or(1 << channel, Ordering::Relaxed);
        }
    }

    fn append(&mut self) -> u32 {
        let c = self.num;
        self.num += 1;
        c
    }
}

#[derive(Default)]
struct Message {
    has_sender: bool,
    num_listeners: u32,
    lock: AtomicU64,
    msg: AtomicU8,
}

impl Message {
    pub fn open_lock(&self) -> u64 {
        (u64::MAX << self.num_listeners) >> self.num_listeners
    }

    pub fn send(&self, v: u8) {
        while self.lock.load(Ordering::Relaxed) != self.open_lock() { }
        self.msg.store(v, Ordering::Relaxed);
        self.lock.store(0, Ordering::Relaxed);
    }

    pub fn recieve(&self, channel: u32) -> u8 {
        while self.lock.load(Ordering::Relaxed) & (1 << channel) == 1 { }
        self.lock.fetch_or(1 << channel, Ordering::Relaxed);
        self.msg.load(Ordering::Relaxed)
    }
}

const MAX_SENDABLE: usize = (b'Z' - b'A') as usize;

struct Code {
    instructions: Vec<Vec<PInstr>>,
    stops: Vec<Stop>,
    sends: Vec<[Message; MAX_SENDABLE]>,
}

impl Code {
    fn parse(code: &str) -> Result<Code> {
        let mut stops = Vec::new();

        let mut sends = Vec::new();

        let mut instrs = Vec::new();

        for (y, line) in code.lines().enumerate() {
            let mut instructions = Vec::<PInstr>::new();
            let mut loops = Vec::new();

            let mut neg_last = false;

            for (x, c) in line.chars().enumerate() {
                if x >= stops.len() {
                    stops.push(Stop::default());
                    sends.push(array_init(|_| Message::default()));
                }
                let stop = &mut stops[x];
                let sends = &mut sends[x];
                let error = |kind| Error { kind, c, x, y };
                let err = |kind| Err(error(kind));

                let neg = neg_last;
                neg_last = false;

                let instr = if neg {
                    match c {
                        '0' => IInstr::SubConst(10),
                        '1'..='9' => IInstr::SubConst(c as u8 - b'0'),
                        _ => {
                            return err(ErrorKind::ExpectedNumber);
                        }
                    }
                } else {
                    match c {
                        '0' => IInstr::AddConst(10),
                        '1'..='9' => IInstr::AddConst(c as u8 - b'0'),
                        '-' => { neg_last = true; continue; },
                        '.' => IInstr::Print,
                        ',' => IInstr::Read,
                        '<' => IInstr::Last,
                        '>' => IInstr::Next,
                        '[' => {
                            loops.push(instructions.len());
                            IInstr::StartLoop(0)
                        }
                        ']' => {
                            let start = loops.pop().ok_or(error(ErrorKind::MismatchedLoop))?;
                            let e = instructions.len();
                            let IInstr::StartLoop(ref mut end) = instructions[start].kind else {
                                // Can't be reached because values we push into loops will always be StartLoop.
                                unreachable!()
                            };
                            *end = e;
                            IInstr::EndLoop(start + 1)
                        }
                        '|' => IInstr::Wait(x, stop.append()),
                        'A'..='Z' => {
                            let b = (c as u8 - b'A') as usize;
                            if sends[b].has_sender {
                                return err(ErrorKind::DuplicateSend);
                            }
                            sends[b].has_sender = true;
                            IInstr::Send(x, b)
                        }
                        'a'..='z' => {
                            let b = (c as u8 - b'a') as usize;
                            let channel = sends[b].num_listeners;
                            sends[b].num_listeners += 1;
                            IInstr::Recieve(x, (c as u8 - b'a') as usize, channel)
                        }
                        ' ' => continue,
                        _ => return err(ErrorKind::UnknownInstr),
                    }
                };

                
                instructions.push(PInstr {
                    y,
                    x,
                    kind: instr,
                    c,
                });
            }

            for l in loops {
                return Err(Error {
                    kind: ErrorKind::MismatchedLoop,
                    c: '[',
                    x: l,
                    y,
                });
            }
            instrs.push(instructions);
        }

        for send in &mut sends {
            for msg in send {
                *msg.lock.get_mut() = msg.open_lock();
            }
        }

        for instructions in &instrs {
            for instr in instructions {
                match instr.kind {
                    IInstr::Recieve(i, b, _) => {
                        sends[i][b]
                            .has_sender
                            .then_some(())
                            .ok_or(instr.error(ErrorKind::ReadWithoutSend))?;
                    }
                    IInstr::Send(i, b) => {
                        (sends[i][b].num_listeners > 0)
                            .then_some(())
                            .ok_or(instr.error(ErrorKind::SendWithoutRead))?;
                        (sends[i][b].num_listeners <= 64)
                            .then_some(())
                            .ok_or(instr.error(ErrorKind::TooManyReaders))?;
                    }
                    IInstr::Wait(i, _) => {
                        (stops[i].num > 1)
                            .then_some(())
                            .ok_or(instr.error(ErrorKind::SingleWait))?;
                        (stops[i].num <= 64)
                            .then_some(())
                            .ok_or(instr.error(ErrorKind::TooManyWaits))?;
                    }
                    _ => {}
                }
            }
        }

        Ok(Code {
            instructions: instrs,
            stops,
            sends,
        })
    }
}

struct Program<'a> {
    lanes: Vec<Lane<'a>>,
}

#[derive(Debug)]
enum ErrorKind {
    UnknownInstr,
    ExpectedNumber,
    MismatchedLoop,
    DuplicateSend,
    ReadWithoutSend,
    TooManyReaders,
    SendWithoutRead,
    SingleWait,
    TooManyWaits,
}

#[allow(dead_code)]
#[derive(Debug)]
struct Error {
    kind: ErrorKind,
    c: char,
    x: usize,
    y: usize,
}

type Result<T> = std::result::Result<T, Error>;

impl<'a> Program<'a> {
    fn parse(code: &'a Code) -> Program<'a> {
        let mut lanes = Vec::new();

        for instructions in &code.instructions {
            let instructions = instructions
                .into_iter()
                .map(|instr| match instr.kind {
                    IInstr::Last => Instr::Last,
                    IInstr::Next => Instr::Next,
                    IInstr::AddConst(n) => Instr::AddConst(n),
                    IInstr::SubConst(n) => Instr::SubConst(n),
                    IInstr::Print => Instr::Print,
                    IInstr::Read => Instr::Read,
                    IInstr::StartLoop(n) => Instr::StartLoop(n),
                    IInstr::EndLoop(n) => Instr::EndLoop(n),
                    IInstr::Wait(n, c) => Instr::Wait(&code.stops[n], c),
                    IInstr::Send(i, b) => Instr::Send(&code.sends[i][b]),
                    IInstr::Recieve(i, b, c) => Instr::Recieve(&code.sends[i][b], c),
                })
                .collect::<Vec<_>>();

            lanes.push(Lane { instructions });
        }

        Program { lanes }
    }

    fn dispatch(&self) {
        crossbeam::scope(|s| {
            for lane in &self.lanes {
                s.spawn(move |_| {
                    lane.run();
                });
            }
        })
        .expect("Failed to run scope");
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if let Some(code) = args.get(1).and_then(|file| fs::read_to_string(file).ok()) {
        let code = Code::parse(&code).unwrap();
        let program = Program::parse(&code);
        program.dispatch();
    } else {
        println!("Failed to read file")
    }
}
