/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Implementation of the GDB Remote Serial Protocol. This implements a server;
//! the client would be something like GDB or LLDB.
//!
//! Useful resources:
//! - [Debugging with GDB, Appendix E: GDB Remote Serial Protocol](https://sourceware.org/gdb/onlinedocs/gdb/Remote-Protocol.html)
//! - The GDB source code:
//!   - `include/gdb/signals.def` for the meanings of signal numbers
//!   - `gdb/arch/arm.h` for ARMv6 register numbers

use crate::cpu::CpuError;
use crate::environment::{Environment, ThreadId};
use crate::mem::{GuestUSize, Ptr};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// GDB target description XML.
const TARGET_XML: &str = r#"
<target version="1.0">
    <architecture>armv6</architecture>
    <osabi>Darwin</osabi>
</target>
"#;

/// GDB Remote Serial Protocol handler, implementing a server.
pub struct GdbServer {
    reader: BufReader<TcpStream>,
    first_halt: bool,
    general_thread: Option<ThreadId>,
    continue_thread: Option<ThreadId>,
    resume_thread: Option<ThreadId>,
}

impl GdbServer {
    /// Create the handler from a TCP connection.
    pub fn new(mut connection: TcpStream) -> GdbServer {
        connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        connection
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        let mut hello_byte = [0u8; 1];
        connection
            .read_exact(&mut hello_byte)
            .expect("Could not read greeting");
        assert!(hello_byte[0] == b'+');

        connection.write_all(b"+").expect("Could not send greeting");

        GdbServer {
            reader: BufReader::with_capacity(4096, connection),
            first_halt: true,
            general_thread: None,
            continue_thread: None,
            resume_thread: None,
        }
    }

    fn read_packet(&mut self) -> Option<String> {
        let buffer = match self.reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(e) => match e.kind() {
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset => {
                    panic!("Lost connection to debugger: {}", e.kind());
                }
                _ => return None,
            },
        };

        if buffer.is_empty() {
            return None;
        }

        // Packets begin with '$', followed by the main content, followed by
        // '#', followed by a two-digit checksum in hexadecimal.
        // Except when some optional extensions are enabled, the content is
        // always ASCII.

        if buffer[0] == b'+' {
            // This is just an acknowledgment
            self.reader.consume(1);
            log_dbg!("Got ACK");
            return None;
        }

        // This is a normal packet
        assert_eq!(buffer[0], b'$');

        let Some(body_end) = buffer.iter().position(|&c| c == b'#') else {
            // Assumption: packet will never be longer than the maximum buffer
            // size, so if the buffer's full and we don't find a terminator, the
            // data must be invalid or we've parsed it wrong.
            assert!(buffer.len() != self.reader.capacity());
            log_dbg!("No packet end yet");
            return None;
        };

        let body = &buffer[1..body_end];

        let checksum1 = buffer.get((body_end + 1)..(body_end + 3))?;
        log_dbg!("Have full packet");

        let checksum1 = std::str::from_utf8(checksum1).unwrap();
        let checksum1 = u8::from_str_radix(checksum1, 16).unwrap();
        let checksum2 = body.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        assert_eq!(checksum1, checksum2);

        let body = String::from_utf8(body.to_vec()).unwrap();
        self.reader.consume(body_end + 3);

        log_dbg!("Got packet: {:?}", body);

        // Send acknowledgment
        self.reader
            .get_mut()
            .write_all(b"+")
            .expect("Couldn't send ACK");

        Some(body)
    }

    fn send_packet(&mut self, body: &str) {
        let checksum = body.bytes().fold(0u8, |a, b| a.wrapping_add(b));
        write!(self.reader.get_mut(), "${body}#{checksum:02x}").unwrap();
        log_dbg!("Sent packet: {:?}", body);
    }

    fn thread_is_live(env: &Environment, thread_id: ThreadId) -> bool {
        env.threads.get(thread_id).is_some_and(|thread| thread.active)
    }

    fn wire_thread_id(thread_id: ThreadId) -> String {
        format!("{:x}", thread_id.saturating_add(1))
    }

    fn parse_thread_selector(
        spec: &str,
        env: &Environment,
    ) -> Result<Option<ThreadId>, ()> {
        if spec == "-1" || spec == "0" {
            return Ok(None);
        }
        let wire_id = u64::from_str_radix(spec, 16).map_err(|_| ())?;
        let thread_id = wire_id
            .checked_sub(1)
            .and_then(|id| usize::try_from(id).ok())
            .ok_or(())?;
        if Self::thread_is_live(env, thread_id) {
            Ok(Some(thread_id))
        } else {
            Err(())
        }
    }

    fn selected_general_thread(&self, env: &Environment) -> ThreadId {
        self.general_thread
            .filter(|&thread_id| Self::thread_is_live(env, thread_id))
            .unwrap_or(env.current_thread)
    }

    fn selected_continue_thread(&self, env: &Environment) -> ThreadId {
        self.continue_thread
            .filter(|&thread_id| Self::thread_is_live(env, thread_id))
            .unwrap_or(env.current_thread)
    }

    fn registers_for_thread(env: &Environment, thread_id: ThreadId) -> Option<([u32; 16], u32)> {
        if thread_id == env.current_thread {
            return Some((*env.cpu.regs(), env.cpu.cpsr()));
        }
        env.threads
            .get(thread_id)
            .and_then(|thread| thread.guest_context.as_ref())
            .map(|context| (context.regs, context.cpsr))
    }

    fn write_registers_for_thread(
        env: &mut Environment,
        thread_id: ThreadId,
        regs: [u32; 16],
        cpsr: u32,
    ) -> bool {
        if thread_id == env.current_thread {
            env.cpu.regs_mut().copy_from_slice(&regs);
            env.cpu.set_cpsr(cpsr);
            return true;
        }
        let Some(context) = env
            .threads
            .get_mut(thread_id)
            .and_then(|thread| thread.guest_context.as_mut())
        else {
            return false;
        };
        context.regs = regs;
        context.cpsr = cpsr;
        true
    }

    fn set_pc_for_thread(env: &mut Environment, thread_id: ThreadId, address: u32) -> bool {
        let function = crate::abi::GuestFunction::from_addr_with_thumb_bit(address);
        if thread_id == env.current_thread {
            env.cpu.branch(function);
            return true;
        }
        let Some(context) = env
            .threads
            .get_mut(thread_id)
            .and_then(|thread| thread.guest_context.as_mut())
        else {
            return false;
        };
        context.regs[crate::cpu::Cpu::PC] = function.addr_without_thumb_bit();
        context.cpsr = (context.cpsr & !crate::cpu::Cpu::CPSR_THUMB)
            | ((function.is_thumb() as u32) * crate::cpu::Cpu::CPSR_THUMB);
        true
    }

    fn send_stop_reply(&mut self, signal: &str, env: &Environment) {
        self.send_packet(&format!(
            "{signal};thread:{};",
            Self::wire_thread_id(env.current_thread)
        ));
    }

    /// Return the thread selected by the next continue/step command, if GDB
    /// explicitly selected one with `Hc` or `vCont`.
    pub fn take_resume_thread(&mut self) -> Option<ThreadId> {
        self.resume_thread.take()
    }

    /// Communciates with the debugger, returning only once it requests
    /// execution should continue. Returns [true] if the CPU should step and
    /// then resume debugging, or [false] if it should resume normal execution.
    #[must_use]
    pub fn wait_for_debugger(
        &mut self,
        stop_reason: Option<CpuError>,
        env: &mut Environment,
    ) -> bool {
        echo!("Waiting for debugger to continue.");

        match stop_reason {
            None if self.first_halt => {
                self.first_halt = false;
            }
            None => self.send_stop_reply("S05", env),
            Some(CpuError::UndefinedInstruction) | Some(CpuError::Breakpoint) => {
                self.send_stop_reply("S05", env)
            }
            Some(CpuError::MemoryError) => self.send_stop_reply("S0b", env),
        }

        let do_step = loop {
            let Some(packet) = self.read_packet() else {
                continue;
            };
            if packet.is_empty() {
                continue;
            }

            match packet.as_bytes()[0] {
                b'?' => {
                    let signal = match stop_reason {
                        Some(CpuError::MemoryError) => "S0b",
                        Some(CpuError::UndefinedInstruction) | Some(CpuError::Breakpoint) => "S05",
                        None => "S00",
                    };
                    self.send_stop_reply(signal, env);
                }
                b'g' => {
                    let thread_id = self.selected_general_thread(env);
                    let Some((regs, _cpsr)) = Self::registers_for_thread(env, thread_id) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let mut response = String::with_capacity(regs.len() * 8);
                    for reg in regs {
                        write!(response, "{:08x}", u32::from_be_bytes(reg.to_le_bytes())).unwrap();
                    }
                    self.send_packet(&response);
                }
                b'G' => {
                    let data = &packet[1..];
                    if data.len() != 16 * 8 {
                        self.send_packet("E01");
                        continue;
                    }
                    let mut regs = [0u32; 16];
                    let mut valid = true;
                    for (index, reg) in regs.iter_mut().enumerate() {
                        let word = &data[index * 8..index * 8 + 8];
                        match u32::from_str_radix(word, 16) {
                            Ok(value) => *reg = u32::from_le_bytes(value.to_be_bytes()),
                            Err(_) => {
                                valid = false;
                                break;
                            }
                        }
                    }
                    if !valid {
                        self.send_packet("E01");
                        continue;
                    }
                    let thread_id = self.selected_general_thread(env);
                    let Some((_, cpsr)) = Self::registers_for_thread(env, thread_id) else {
                        self.send_packet("E01");
                        continue;
                    };
                    if Self::write_registers_for_thread(env, thread_id, regs, cpsr) {
                        self.send_packet("OK");
                    } else {
                        self.send_packet("E01");
                    }
                }
                b'p' => {
                    let Ok(number) = usize::from_str_radix(&packet[1..], 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let thread_id = self.selected_general_thread(env);
                    let Some((regs, cpsr)) = Self::registers_for_thread(env, thread_id) else {
                        self.send_packet("E01");
                        continue;
                    };
                    if (26..=57).contains(&number) {
                        self.send_packet("0000000000000000");
                    } else if number < 16 {
                        let value = u32::from_be_bytes(regs[number].to_le_bytes());
                        self.send_packet(&format!("{value:08x}"));
                    } else if number == 25 {
                        let value = u32::from_be_bytes(cpsr.to_le_bytes());
                        self.send_packet(&format!("{value:08x}"));
                    } else if (16..=24).contains(&number) || number == 58 {
                        self.send_packet("00000000");
                    } else {
                        self.send_packet("E00");
                    }
                }
                b'P' => {
                    let Some((number, encoded)) = packet[1..].split_once('=') else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Ok(number) = usize::from_str_radix(number, 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Ok(encoded) = u32::from_str_radix(encoded, 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    if (26..=57).contains(&number) || (16..=24).contains(&number) || number == 58 {
                        self.send_packet("OK");
                        continue;
                    }
                    let thread_id = self.selected_general_thread(env);
                    let Some((mut regs, mut cpsr)) = Self::registers_for_thread(env, thread_id) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let value = u32::from_le_bytes(encoded.to_be_bytes());
                    if number < 16 {
                        regs[number] = value;
                    } else if number == 25 {
                        cpsr = value;
                    } else {
                        self.send_packet("E00");
                        continue;
                    }
                    if Self::write_registers_for_thread(env, thread_id, regs, cpsr) {
                        self.send_packet("OK");
                    } else {
                        self.send_packet("E01");
                    }
                }
                b'm' => {
                    let Some((address, length)) = packet[1..].split_once(',') else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Ok(address) = GuestUSize::from_str_radix(address, 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Ok(length) = GuestUSize::from_str_radix(length, 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    const MAX_GDB_MEMORY_TRANSFER: GuestUSize = 1024 * 1024;
                    if length > MAX_GDB_MEMORY_TRANSFER {
                        self.send_packet("E01");
                        continue;
                    }
                    let Some(data) = env.mem.get_bytes_fallible(Ptr::from_bits(address), length) else {
                        self.send_packet("E00");
                        continue;
                    };
                    let mut response = String::with_capacity(data.len() * 2);
                    for byte in data {
                        write!(response, "{byte:02x}").unwrap();
                    }
                    self.send_packet(&response);
                }
                b'M' => {
                    let Some((header, data)) = packet[1..].split_once(':') else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Some((address, length)) = header.split_once(',') else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Ok(address) = GuestUSize::from_str_radix(address, 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Ok(length) = GuestUSize::from_str_radix(length, 16) else {
                        self.send_packet("E01");
                        continue;
                    };
                    let Some(expected_len) = (length as usize).checked_mul(2) else {
                        self.send_packet("E01");
                        continue;
                    };
                    if data.len() != expected_len || length > 1024 * 1024 {
                        self.send_packet("E01");
                        continue;
                    }
                    let Some(destination) = env.mem.get_bytes_fallible_mut(Ptr::from_bits(address), length) else {
                        self.send_packet("E00");
                        continue;
                    };
                    let mut valid = true;
                    for (index, byte) in data.as_bytes().chunks_exact(2).enumerate() {
                        let Ok(value) = u8::from_str_radix(std::str::from_utf8(byte).unwrap_or(""), 16) else {
                            valid = false;
                            break;
                        };
                        destination[index] = value;
                    }
                    if valid {
                        env.cpu.invalidate_cache_range(address, length);
                        self.send_packet("OK");
                    } else {
                        self.send_packet("E01");
                    }
                }
                b'H' => {
                    if packet.len() < 3 {
                        self.send_packet("E01");
                        continue;
                    }
                    let kind = packet.as_bytes()[1] as char;
                    match Self::parse_thread_selector(&packet[2..], env) {
                        Ok(thread_id) if kind == 'g' => {
                            self.general_thread = thread_id;
                            self.send_packet("OK");
                        }
                        Ok(thread_id) if kind == 'c' => {
                            self.continue_thread = thread_id;
                            self.send_packet("OK");
                        }
                        _ => self.send_packet("E01"),
                    }
                }
                b'T' => {
                    match Self::parse_thread_selector(&packet[1..], env) {
                        Ok(Some(_)) => self.send_packet("OK"),
                        Ok(None) if Self::thread_is_live(env, env.current_thread) => {
                            self.send_packet("OK")
                        }
                        _ => self.send_packet("E01"),
                    }
                }
                b'c' | b's' => {
                    let thread_id = self.selected_continue_thread(env);
                    let address = &packet[1..];
                    if !address.is_empty() {
                        let Ok(address) = u32::from_str_radix(address, 16) else {
                            self.send_packet("E01");
                            continue;
                        };
                        if !Self::set_pc_for_thread(env, thread_id, address) {
                            self.send_packet("E01");
                            continue;
                        }
                    }
                    self.resume_thread = self.continue_thread;
                    break packet.as_bytes()[0] == b's';
                }
                b'C' | b'S' => {
                    let thread_id = self.selected_continue_thread(env);
                    if let Some((_signal, address)) = packet[1..].split_once(';') {
                        if !address.is_empty() {
                            let Ok(address) = u32::from_str_radix(address, 16) else {
                                self.send_packet("E01");
                                continue;
                            };
                            if !Self::set_pc_for_thread(env, thread_id, address) {
                                self.send_packet("E01");
                                continue;
                            }
                        }
                    }
                    self.resume_thread = self.continue_thread;
                    break packet.as_bytes()[0] == b'S';
                }
                b'v' if packet == "vCont?" => {
                    self.send_packet("vCont;c;s");
                }
                b'v' if packet.starts_with("vCont;") => {
                    let action = packet[6..].split(';').next().unwrap_or("");
                    let (action, thread_spec) = action.split_once(':').map_or((action, None), |(action, thread)| (action, Some(thread)));
                    if let Some(thread_spec) = thread_spec {
                        match Self::parse_thread_selector(thread_spec, env) {
                            Ok(thread_id) => self.continue_thread = thread_id,
                            Err(()) => {
                                self.send_packet("E01");
                                continue;
                            }
                        }
                    }
                    if action == "c" || action == "s" {
                        self.resume_thread = self.continue_thread;
                        break action == "s";
                    }
                    self.send_packet("E01");
                }
                b'k' => panic!("Debugger requested kill."),
                b'D' => {
                    self.send_packet("OK");
                    break false;
                }
                _ => {
                    if packet == "qAttached" {
                        self.send_packet("0");
                    } else if packet == "qC" {
                        self.send_packet(&format!("QC{}", Self::wire_thread_id(env.current_thread)));
                    } else if packet == "qfThreadInfo" {
                        let ids = env
                            .threads
                            .iter()
                            .enumerate()
                            .filter(|(_, thread)| thread.active)
                            .map(|(id, _)| Self::wire_thread_id(id))
                            .collect::<Vec<_>>();
                        if ids.is_empty() {
                            self.send_packet("l");
                        } else {
                            self.send_packet(&format!("m{}", ids.join(",")));
                        }
                    } else if packet == "qsThreadInfo" {
                        self.send_packet("l");
                    } else if let Some(thread_spec) = packet.strip_prefix("qThreadExtraInfo,") {
                        match Self::parse_thread_selector(thread_spec, env) {
                            Ok(Some(thread_id)) => {
                                let thread = &env.threads[thread_id];
                                let description = format!(
                                    "guest thread {} active={} blocked={:?}",
                                    Self::wire_thread_id(thread_id),
                                    thread.active,
                                    thread.blocked_by
                                );
                                let mut encoded = String::with_capacity(description.len() * 2);
                                for byte in description.bytes() {
                                    write!(encoded, "{byte:02x}").unwrap();
                                }
                                self.send_packet(&encoded);
                            }
                            Ok(None) if Self::thread_is_live(env, env.current_thread) => {
                                self.send_packet("63757272656e7420");
                            }
                            _ => self.send_packet("E01"),
                        }
                    } else if packet == "qSupported" || packet.starts_with("qSupported:") {
                        self.send_packet("PacketSize=1000;qXfer:features:read+;qXfer:threads:read+;multiprocess+;vContSupported+");
                    } else if let Some(params) = packet.strip_prefix("qXfer:features:read:") {
                        let Some((annex, range)) = params.split_once(':') else {
                            self.send_packet("E01");
                            continue;
                        };
                        let Some((offset, length)) = range.split_once(',') else {
                            self.send_packet("E01");
                            continue;
                        };
                        let (Ok(offset), Ok(length)) = (usize::from_str_radix(offset, 16), usize::from_str_radix(length, 16)) else {
                            self.send_packet("E01");
                            continue;
                        };
                        let bytes = TARGET_XML.as_bytes();
                        if annex == "target.xml" && offset <= bytes.len() {
                            let end = offset.saturating_add(length).min(bytes.len());
                            let marker = if end < bytes.len() { 'm' } else { 'l' };
                            let mut response = String::with_capacity(1 + end.saturating_sub(offset));
                            response.push(marker);
                            response.push_str(std::str::from_utf8(&bytes[offset..end]).unwrap_or(""));
                            self.send_packet(&response);
                        } else {
                            self.send_packet("E00");
                        }
                    } else if let Some(params) = packet.strip_prefix("qXfer:threads:read:") {
                        let Some((annex, range)) = params.split_once(':') else {
                            self.send_packet("E01");
                            continue;
                        };
                        let Some((offset, length)) = range.split_once(',') else {
                            self.send_packet("E01");
                            continue;
                        };
                        let (Ok(offset), Ok(length)) = (usize::from_str_radix(offset, 16), usize::from_str_radix(length, 16)) else {
                            self.send_packet("E01");
                            continue;
                        };
                        if !annex.is_empty() && annex != "threads" {
                            self.send_packet("E00");
                            continue;
                        }
                        let mut xml = String::from("<threads>");
                        for (id, thread) in env.threads.iter().enumerate() {
                            if thread.active {
                                write!(xml, "<thread id=\"{}\" name=\"guest-{}\"/>", Self::wire_thread_id(id), id).unwrap();
                            }
                        }
                        xml.push_str("</threads>");
                        let bytes = xml.as_bytes();
                        if offset > bytes.len() {
                            self.send_packet("E00");
                            continue;
                        }
                        let end = offset.saturating_add(length).min(bytes.len());
                        let marker = if end < bytes.len() { 'm' } else { 'l' };
                        let mut response = String::with_capacity(1 + end.saturating_sub(offset));
                        response.push(marker);
                        response.push_str(std::str::from_utf8(&bytes[offset..end]).unwrap_or(""));
                        self.send_packet(&response);
                    } else {
                        log_dbg!("Unhandled GDB packet: {:?}", packet);
                        self.send_packet("");
                    }
                }
            }
        };

        if do_step {
            echo!("Debugger requested step, resuming execution for one instruction only.");
        } else {
            echo!("Debugger requested continue, resuming execution.");
        }
        do_step
    }
}
