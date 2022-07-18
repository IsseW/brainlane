#![feature(let_else)]
use std::{
    env, fs,
    io::{self, Read},
    sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
};

use array_init::array_init;

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
    Wait(usize),
    /// A..=Z
    Send(usize, usize),
    /// a..=z
    Recieve(usize, usize),
}

struct PInstr {
    y: usize,
    x: usize,
    kind: IInstr,
    c: char,
}

impl PInstr {
    fn error(&self, kind: ErrorKind) -> Error {
        Error { kind, c: self.c, x: self.x, y: self.y }
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
    Wait(&'a Stop),
    /// A..=Z
    Send(&'a (AtomicBool, AtomicU8)),
    /// a..=z
    Recieve(&'a (AtomicBool, AtomicU8)),
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
                Instr::Read => mem[ptr] = io::stdin().bytes().next().unwrap().unwrap(),
                Instr::Wait(stop) => {
                    stop.reach();
                    while !stop.is_go() {}
                }
                Instr::Last => {
                    ptr = MEM.min(ptr.wrapping_sub(1));
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
                Instr::Send(send) => {
                    send.1.store(mem[ptr], Ordering::Release);
                    send.0.store(true, Ordering::Release);
                }
                Instr::Recieve(rec) => {
                    while !rec.0.load(Ordering::Relaxed) { }
                    mem[ptr] = rec.1.load(Ordering::Relaxed);
                }
            }

            cptr = new_line.unwrap_or(cptr + 1);
        }
    }
}

#[derive(Default)]
struct Stop {
    current: AtomicU32,
}

impl Stop {
    fn reach(&self) {
        self.current.fetch_sub(1, Ordering::Relaxed);
    }

    fn is_go(&self) -> bool {
        return self.current.load(Ordering::Relaxed) == 0;
    }
}

const MAX_SENDABLE: usize = (b'Z' - b'A') as usize;

struct Code {
    instructions: Vec<Vec<PInstr>>,
    stops: Vec<Stop>,
    sends: Vec<[Option<(AtomicBool, AtomicU8)>; MAX_SENDABLE]>,
}

impl Code {
    fn parse(code: &str) -> Result<Code> {
        let mut stops = Vec::new();

        let mut sends = Vec::new();

        let mut instrs = Vec::new();

        for (y, line) in code.lines().enumerate() {
            let mut instructions = Vec::<PInstr>::new();
            let mut loops = Vec::new();

            for (x, c) in line.chars().enumerate() {
                if x >= stops.len() {
                    stops.push(Stop::default());
                    sends.push(array_init(|_| None));
                }
                let num_stops = &mut stops[x].current;
                let sends = &mut sends[x];
                let error = |kind| Error { kind, c, x, y };
                let err = |kind| Err(error(kind));

                let instr = match c {
                    '0' => IInstr::AddConst(10),
                    '1'..='9' => IInstr::AddConst(c as u8 - b'0'),
                    '-' => IInstr::SubConst(1),
                    '+' => IInstr::AddConst(1),
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
                    '|' => {
                        num_stops.fetch_add(1, Ordering::Relaxed);
                        IInstr::Wait(x)
                    }
                    'A'..='Z' => {
                        let b = (c as u8 - b'A') as usize;
                        if sends[b].is_some() {
                            return err(ErrorKind::DuplicateSend);
                        }
                        sends[b].replace((AtomicBool::new(false), AtomicU8::new(0)));
                        IInstr::Send(x, b)
                    }
                    'a'..='z' => IInstr::Recieve(x, (c as u8 - b'a') as usize),
                    ' ' => continue,
                    _ => return err(ErrorKind::UnknownInstr),
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

        for instructions in &instrs {
            for instr in instructions {
                match instr.kind {
                    IInstr::Recieve(i, b) => {
                        sends[i][b].as_ref().ok_or(instr.error(ErrorKind::ReadWithoutSend))?;
                    },
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
    MismatchedLoop,
    DuplicateSend,
    ReadWithoutSend,
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
                    IInstr::Wait(n) => Instr::Wait(&code.stops[n]),
                    IInstr::Send(i, b) => Instr::Send(code.sends[i][b].as_ref().unwrap()),
                    IInstr::Recieve(i, b) => Instr::Recieve(code.sends[i][b].as_ref().unwrap()),
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
