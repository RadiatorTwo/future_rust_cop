//! Compiling the whole program with Cranelift's JIT: every function is declared up front
//! (so calls between them are direct), lifted, compiled, and entered through a trampoline.

use crate::discover::{Code, Program};
use crate::lift::{Lifter, Refs};
use crate::rt::{self, Cpu, DISPATCH, Dispatch};
use crate::smc::Smc;
use cranelift_codegen::ir::{AbiParam, InstBuilder, Signature, UserFuncName, types::*};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

pub struct Compiled {
    _module: JITModule,
    enter: extern "C" fn(*mut Cpu, *const u8),
    pub fns: HashMap<u32, *const u8>,
    pub stats: Stats,
}

#[derive(Default)]
pub struct Stats {
    pub functions: usize,
    pub complete: usize,
    pub instructions: usize,
    pub missing: BTreeMap<String, usize>,
    pub failed: Vec<(u32, String)>,
    pub lift_ms: u128,
    pub compile_ms: u128,
}

impl Compiled {
    /// Call the guest function at `addr` (its return address must already be on the guest stack).
    pub fn call(&self, cpu: &mut Cpu, addr: u32) {
        let f = self.fns[&addr];
        (self.enter)(cpu, f);
    }
}

pub fn compile(prog: &Program, smc: &Smc, code: &mut Code) -> Compiled {
    let mut flags = settings::builder();
    flags.set("opt_level", "speed").unwrap();
    flags.set("is_pic", "false").unwrap();
    flags.set("use_colocated_libcalls", "false").unwrap();
    flags.set("preserve_frame_pointers", "true").unwrap();
    let isa = cranelift_native::builder()
        .expect("host not supported by Cranelift")
        .finish(settings::Flags::new(flags))
        .expect("Cranelift settings");
    let host_cc = isa.default_call_conv();
    let mut jb = JITBuilder::with_isa(isa, default_libcall_names());
    jb.symbol("rt_lookup", rt::rt_lookup as *const u8);
    jb.symbol("rt_foreign", rt::rt_foreign as *const u8);
    jb.symbol("rt_unimpl", rt::rt_unimpl as *const u8);
    jb.symbol("rt_trap", rt::rt_trap as *const u8);
    jb.symbol("rt_string", rt::rt_string as *const u8);
    jb.symbol("rt_cpuid", rt::rt_cpuid as *const u8);
    jb.symbol("rt_rdtsc", rt::rt_rdtsc as *const u8);
    jb.symbol("rt_x87", rt::rt_x87 as *const u8);
    jb.symbol("rt_fpu_round", rt::rt_fpu_round as *const u8);
    let mut module = JITModule::new(jb);
    let tcfg = module.target_config();
    let ptr = tcfg.pointer_type();

    let sig = |cc: CallConv, params: &[cranelift_codegen::ir::Type], ret: Option<cranelift_codegen::ir::Type>| {
        let mut s = Signature::new(cc);
        s.params.extend(params.iter().map(|&t| AbiParam::new(t)));
        s.returns.extend(ret.map(AbiParam::new));
        s
    };
    // guest functions: fn(cpu) with the tail calling convention (return_call between them)
    let guest_sig = sig(CallConv::Tail, &[ptr], None);
    let mut decl = |name: &str, s: &Signature| module.declare_function(name, Linkage::Import, s).unwrap();
    let h_lookup = decl("rt_lookup", &sig(host_cc, &[ptr, I32], Some(ptr)));
    let h_foreign = decl("rt_foreign", &sig(host_cc, &[ptr], None));
    let h_unimpl = decl("rt_unimpl", &sig(host_cc, &[ptr, I32], None));
    let h_trap = decl("rt_trap", &sig(host_cc, &[ptr, I32, I32], None));
    let h_string = decl("rt_string", &sig(host_cc, &[ptr, I32], None));
    let h_cpuid = decl("rt_cpuid", &sig(host_cc, &[ptr], None));
    let h_rdtsc = decl("rt_rdtsc", &sig(host_cc, &[ptr], None));
    let h_x87 = decl("rt_x87", &sig(host_cc, &[ptr, I32, I32], None));
    let h_fpu_round = decl("rt_fpu_round", &sig(host_cc, &[I32, F64, F64, I32], Some(F64)));

    let ids: BTreeMap<u32, FuncId> = prog
        .fns
        .keys()
        .map(|&a| (a, module.declare_function(&format!("F_{a:x}"), Linkage::Local, &guest_sig).unwrap()))
        .collect();
    let foreign_id = module.declare_function("foreign_stub", Linkage::Local, &guest_sig).unwrap();
    let enter_id = module.declare_function("enter", Linkage::Local, &sig(host_cc, &[ptr, ptr], None)).unwrap();

    let mut stats = Stats { functions: prog.fns.len(), ..Default::default() };
    let mut ctx = module.make_context();
    let mut fctx = FunctionBuilderContext::new();
    let t0 = Instant::now();
    let mut lift_time = std::time::Duration::ZERO;

    for (&addr, f) in &prog.fns {
        let id = ids[&addr];
        ctx.func.signature = guest_sig.clone();
        ctx.func.name = UserFuncName::user(0, id.as_u32());
        let t = Instant::now();
        let missing = {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            let refs = Refs {
                lookup: module.declare_func_in_func(h_lookup, b.func),
                unimpl: module.declare_func_in_func(h_unimpl, b.func),
                trap: module.declare_func_in_func(h_trap, b.func),
                string: module.declare_func_in_func(h_string, b.func),
                cpuid: module.declare_func_in_func(h_cpuid, b.func),
                rdtsc: module.declare_func_in_func(h_rdtsc, b.func),
                x87: module.declare_func_in_func(h_x87, b.func),
                fpu_round: module.declare_func_in_func(h_fpu_round, b.func),
                guest_sig: b.import_signature(guest_sig.clone()),
            };
            let mut get_fn = |func: &mut cranelift_codegen::ir::Function, a: u32| module.declare_func_in_func(ids[&a], func);
            Lifter::new(b, f, prog, smc, &refs, &mut get_fn, tcfg).lift(code)
        };
        lift_time += t.elapsed();
        stats.instructions += f.ins.len();
        if missing.is_empty() {
            stats.complete += 1;
        }
        for m in missing {
            *stats.missing.entry(m).or_default() += 1;
        }
        if let Err(e) = module.define_function(id, &mut ctx) {
            stats.failed.push((addr, format!("{e:?}")));
            module.clear_context(&mut ctx);
            // keep the program linkable: this function only reports itself
            ctx.func.signature = guest_sig.clone();
            ctx.func.name = UserFuncName::user(0, id.as_u32());
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            let unimpl = module.declare_func_in_func(h_unimpl, b.func);
            let blk = b.create_block();
            b.append_block_params_for_function_params(blk);
            b.switch_to_block(blk);
            let cpu = b.block_params(blk)[0];
            let a = b.ins().iconst(I32, addr as i64);
            b.ins().call(unimpl, &[cpu, a]);
            b.ins().return_(&[]);
            b.seal_all_blocks();
            b.finalize(tcfg);
            module.define_function(id, &mut ctx).expect("stub function");
        }
        module.clear_context(&mut ctx);
    }

    // foreign_stub(cpu): rt_foreign(cpu) -- the target of every call that is not recompiled code
    ctx.func.signature = guest_sig.clone();
    ctx.func.name = UserFuncName::user(0, foreign_id.as_u32());
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
        let f = module.declare_func_in_func(h_foreign, b.func);
        let blk = b.create_block();
        b.append_block_params_for_function_params(blk);
        b.switch_to_block(blk);
        let cpu = b.block_params(blk)[0];
        b.ins().call(f, &[cpu]);
        b.ins().return_(&[]);
        b.seal_all_blocks();
        b.finalize(tcfg);
    }
    module.define_function(foreign_id, &mut ctx).unwrap();
    module.clear_context(&mut ctx);

    // enter(cpu, fn): the host calls guest code (tail convention) through this
    ctx.func.signature = sig(host_cc, &[ptr, ptr], None);
    ctx.func.name = UserFuncName::user(0, enter_id.as_u32());
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
        let gs = b.import_signature(guest_sig.clone());
        let blk = b.create_block();
        b.append_block_params_for_function_params(blk);
        b.switch_to_block(blk);
        let (cpu, f) = (b.block_params(blk)[0], b.block_params(blk)[1]);
        b.ins().call_indirect(gs, f, &[cpu]);
        b.ins().return_(&[]);
        b.seal_all_blocks();
        b.finalize(tcfg);
    }
    module.define_function(enter_id, &mut ctx).unwrap();
    module.clear_context(&mut ctx);

    module.finalize_definitions().expect("linking the JIT code");
    stats.lift_ms = lift_time.as_millis();
    stats.compile_ms = t0.elapsed().as_millis() - stats.lift_ms;

    let fns: HashMap<u32, *const u8> = ids.iter().map(|(&a, &id)| (a, module.get_finalized_function(id))).collect();
    let mut table: Vec<(u32, usize)> = fns.iter().map(|(&a, &p)| (a, p as usize)).collect();
    table.sort_unstable();
    let foreign_stub = module.get_finalized_function(foreign_id) as usize;
    let enter = unsafe {
        std::mem::transmute::<*const u8, extern "C" fn(*mut Cpu, *const u8)>(module.get_finalized_function(enter_id))
    };
    let _ = DISPATCH.set(Dispatch { table, foreign_stub, enter });
    Compiled { _module: module, enter, fns, stats }
}
