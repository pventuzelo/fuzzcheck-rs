use crate::fuzzer::code_coverage_sensor::*;
use std::sync::Once;
use std::slice;

extern "C" {
    fn return_address() -> usize;
}

static START: Once = Once::new();

#[export_name="__sanitizer_cov_trace_pc_guard_init"]
fn trace_pc_guard_init(start: *mut u32, stop: *mut u32) {	
	unsafe {
		START.call_once(|| {
			SHARED_SENSOR.as_mut_ptr().write(
				CodeCoverageSensor {
					num_guards: 0,
					is_recording: false,
					eight_bit_counters: Vec::with_capacity(0),
					cmp_features: Vec::new()
				}
			);
		});
	}
	shared_sensor().handle_pc_guard_init(start, stop);
}

#[export_name="__sanitizer_cov_trace_pc_guard"]
fn trace_pc_guard(pc: *mut u32) {
	let sensor = shared_sensor();
	if sensor.is_recording == false { return }
	// TODO: check
	let idx = unsafe { *pc as usize };
	// TODO: overflow check
	sensor.eight_bit_counters[idx] += 1;
}

#[export_name="__sanitizer_cov_trace_cmp1"]
fn trace_cmp1(arg1: u8, arg2: u8) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_cmp2"]
fn trace_cmp2(arg1: u16, arg2: u16) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_cmp4"]
fn trace_cmp4(arg1: u32, arg2: u32) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_cmp8"]
fn trace_cmp8(arg1: u64, arg2: u64) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_const_cmp1"]
fn trace_const_cmp1(arg1: u8, arg2: u8) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_const_cmp2"]
fn trace_const_cmp2(arg1: u16, arg2: u16) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_const_cmp4"]
fn trace_const_cmp4(arg1: u32, arg2: u32) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_const_cmp8"]
fn trace_const_cmp8(arg1: u64, arg2: u64) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    sensor.handle_trace_cmp(pc, arg1 as u64, arg2 as u64);
}

#[export_name="__sanitizer_cov_trace_switch"]
fn trace_switch(val: u64, arg2: *mut u64) {
    let sensor = shared_sensor();
	if sensor.is_recording == false { return }
    let pc = unsafe { return_address() };
    
    let n = unsafe { *arg2 as usize };
    let mut cases = unsafe { slice::from_raw_parts_mut(arg2, n+2).iter().take(1) };
    
    // val_size_in_bits
    let _ = cases.next();
    
    // TODO: understand this. actually, understand this whole method
    // if cases[n-1] < 256 && val < 256 { return }

    let (i, token) = cases
        .take_while(|&&x| x <= val) // TODO: not sure this is correct
        .fold((0 as usize, 0 as u64), |x, next| (x.0 + 1, val ^ *next));

    sensor.handle_trace_cmp(pc + i, token, 0);
}

