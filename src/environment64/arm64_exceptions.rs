pub(super) fn handle_arm64_exception(
    reason: &str,
    pc: u64,
    sp: u64,
    fault_address: Option<u64>,
    instruction: u32,
) {
    let class = match reason {
        "memory abort" => match fault_address {
            Some(address) if address < 0x1000 => "null-page read/write",
            Some(_) => "guest memory access",
            None => "unknown memory abort",
        },
        "undefined instruction" => "undefined instruction",
        "illegal instruction" => "illegal instruction",
        "host callback" => "host callback failure",
        _ => "runtime failure",
    };
    let fault = fault_address
        .map(|address| format!("{address:#x}"))
        .unwrap_or_else(|| "<none>".to_owned());
    log!(
        "ARM64 exception: class={} reason={} pc={:#x} sp={:#x} instruction={:#010x} fault_address={}",
        class,
        reason,
        pc,
        sp,
        instruction,
        fault
    );
}
