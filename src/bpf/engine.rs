//! Agave-backed compatibility engine for the `bpf-execution` feature.
//!
//! Older `hopper-svm` callers registered ELFs with methods such as
//! `add_program_from_bytes` and `with_program_loader`. Those method
//! names remain, but the implementation no longer invokes a direct
//! VM shim. Program bytes are loaded through
//! `solana-bpf-loader-program` into an Agave `ProgramCacheForTxBatch`,
//! and execution runs through `solana-program-runtime`.

use crate::account::KeyedAccount;
use crate::agave::{AgaveEngine, AgaveEngineError, AgaveProgramKind};
use crate::compute::ComputeBudget;
use crate::engine::ExecutionOutcome;
use crate::error::HopperSvmError;
use crate::log::LogCapture;
use crate::sysvar::Sysvars;
use solana_program_runtime::execution_budget::{
    SVMTransactionExecutionBudget, SVMTransactionExecutionCost,
};
use solana_sdk::account::{Account as SolanaAccount, ReadableAccount};
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Which BPF loader owns a registered program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoaderKind {
    /// Non-upgradeable BPF Loader v2.
    V2,
    /// Upgradeable BPF Loader v3.
    V3,
}

impl Default for LoaderKind {
    fn default() -> Self {
        Self::V3
    }
}

/// Bundled program record: ELF bytes plus the loader they target.
#[derive(Clone, Debug)]
pub struct LoadedProgram {
    /// Raw ELF (`.so` contents).
    pub elf: Vec<u8>,
    /// Which loader the program is registered against.
    pub loader: LoaderKind,
    /// Loader error captured at registration time. Stored so
    /// compatibility APIs that return `Self` can still surface a
    /// clear execution failure instead of pretending the program ran.
    pub load_error: Option<String>,
}

/// Compatibility BPF execution engine.
#[derive(Clone)]
pub struct BpfEngine {
    programs: Arc<Mutex<HashMap<Pubkey, LoadedProgram>>>,
    agave: AgaveEngine,
}

impl BpfEngine {
    /// Build a fresh engine with no BPF programs loaded. The system
    /// program is installed so BPF programs can CPI to it through
    /// Agave's native builtin path.
    pub fn new() -> Self {
        let agave = AgaveEngine::new();
        agave.install_system_program();
        Self {
            programs: Arc::new(Mutex::new(HashMap::new())),
            agave,
        }
    }

    /// Register the bytes of a `.so` against a program ID. The
    /// loader defaults to [`LoaderKind::V3`].
    pub fn add_elf(&self, program_id: &Pubkey, elf: Vec<u8>) -> Result<(), AgaveEngineError> {
        self.add_elf_with_loader(program_id, elf, LoaderKind::default())
    }

    /// Register a `.so` against a program ID under a specific loader.
    pub fn add_elf_with_loader(
        &self,
        program_id: &Pubkey,
        elf: Vec<u8>,
        loader: LoaderKind,
    ) -> Result<(), AgaveEngineError> {
        let load_result = self.agave.load_bpf_program(
            *program_id,
            agave_kind(loader),
            &loader_key(loader),
            &elf,
            elf.len(),
        );
        let load_error = load_result.as_ref().err().map(ToString::to_string);
        self.programs.lock().expect("programs lock").insert(
            *program_id,
            LoadedProgram {
                elf,
                loader,
                load_error,
            },
        );
        load_result
    }

    /// Read back the loader kind for a registered program.
    pub fn loader_for(&self, program_id: &Pubkey) -> Option<LoaderKind> {
        self.programs
            .lock()
            .expect("programs lock")
            .get(program_id)
            .map(|program| program.loader)
    }

    /// Try to execute an instruction through Agave. Returns `None`
    /// when this engine has no program for `ix.program_id`.
    pub fn try_execute(
        &self,
        ix: &Instruction,
        accounts: &[KeyedAccount],
        budget: &mut ComputeBudget,
        sysvars: &Sysvars,
        logs: &mut LogCapture,
    ) -> Option<ExecutionOutcome> {
        let loaded = self
            .programs
            .lock()
            .expect("programs lock")
            .get(&ix.program_id)
            .cloned()?;

        logs.invoke(&ix.program_id);
        budget.reset();

        if let Some(load_error) = loaded.load_error {
            logs.line(format!("Agave BPF loader failed: {load_error}"));
            return Some(ExecutionOutcome {
                resulting_accounts: accounts.to_vec(),
                compute_units_consumed: 0,
                return_data: Vec::new(),
                inner_instructions: Vec::new(),
                execution_time_us: 0,
                error: Some(HopperSvmError::BuiltinError {
                    program_id: ix.program_id,
                    message: format!(
                        "bpf-execution compatibility load failed through Agave: {load_error}"
                    ),
                }),
            });
        }

        let caller_addresses: Vec<Pubkey> = accounts.iter().map(|ka| ka.address).collect();
        let mut tx_accounts: Vec<(Pubkey, solana_sdk::account::AccountSharedData)> =
            Vec::with_capacity(accounts.len() + 1);
        for ka in accounts {
            let account = SolanaAccount {
                lamports: ka.lamports,
                data: ka.data.clone(),
                owner: ka.owner,
                executable: ka.executable,
                rent_epoch: ka.rent_epoch,
            };
            tx_accounts.push((ka.address, account.into()));
        }

        let program_index = match tx_accounts
            .iter()
            .position(|(key, _)| key == &ix.program_id)
        {
            Some(index) => index as u16,
            None => {
                let mut program_account = SolanaAccount::default();
                program_account.executable = true;
                program_account.owner = loader_key(loaded.loader);
                tx_accounts.push((ix.program_id, program_account.into()));
                (tx_accounts.len() - 1) as u16
            }
        };

        let sysvar_cache = AgaveEngine::build_sysvar_cache(sysvars);
        let mut execution_budget = SVMTransactionExecutionBudget::default();
        execution_budget.compute_unit_limit = budget.limit();

        match self.agave.process_instruction_raw(
            ix,
            tx_accounts,
            vec![program_index],
            &sysvar_cache,
            execution_budget,
            SVMTransactionExecutionCost::default(),
            solana_sdk::rent::Rent::default(),
        ) {
            Ok((cu, post)) => {
                logs.line(format!(
                    "Program {} consumed {cu} compute units (agave-bpf)",
                    ix.program_id
                ));
                let resulting_accounts = post
                    .into_iter()
                    .filter(|(key, _)| caller_addresses.contains(key))
                    .map(|(address, account)| KeyedAccount {
                        address,
                        lamports: account.lamports(),
                        data: account.data().to_vec(),
                        owner: *account.owner(),
                        executable: account.executable(),
                        rent_epoch: account.rent_epoch(),
                    })
                    .collect();
                Some(ExecutionOutcome {
                    resulting_accounts,
                    compute_units_consumed: cu,
                    return_data: Vec::new(),
                    inner_instructions: Vec::new(),
                    execution_time_us: 0,
                    error: None,
                })
            }
            Err(err) => {
                logs.line(format!("agave-bpf: {err}"));
                Some(ExecutionOutcome {
                    resulting_accounts: accounts.to_vec(),
                    compute_units_consumed: 0,
                    return_data: Vec::new(),
                    inner_instructions: Vec::new(),
                    execution_time_us: 0,
                    error: Some(HopperSvmError::BuiltinError {
                        program_id: ix.program_id,
                        message: format!("{err}"),
                    }),
                })
            }
        }
    }
}

impl Default for BpfEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn loader_key(loader: LoaderKind) -> Pubkey {
    match loader {
        LoaderKind::V2 => solana_sdk::bpf_loader::id(),
        LoaderKind::V3 => solana_sdk::bpf_loader_upgradeable::id(),
    }
}

fn agave_kind(loader: LoaderKind) -> AgaveProgramKind {
    match loader {
        LoaderKind::V2 => AgaveProgramKind::BpfV2,
        LoaderKind::V3 => AgaveProgramKind::BpfV3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_execute_returns_none_when_elf_missing() {
        let engine = BpfEngine::new();
        let mut budget = ComputeBudget::default();
        let mut logs = LogCapture::default();
        let sysvars = Sysvars::default();
        let ix = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![],
            data: vec![],
        };
        let result = engine.try_execute(&ix, &[], &mut budget, &sysvars, &mut logs);
        assert!(result.is_none());
    }

    #[test]
    fn malformed_elf_is_remembered_as_execution_error() {
        let engine = BpfEngine::new();
        let program_id = Pubkey::new_unique();
        let _ = engine.add_elf(&program_id, vec![0; 64]);
        let mut budget = ComputeBudget::default();
        let mut logs = LogCapture::default();
        let sysvars = Sysvars::default();
        let ix = Instruction {
            program_id,
            accounts: vec![],
            data: vec![],
        };
        let outcome = engine
            .try_execute(&ix, &[], &mut budget, &sysvars, &mut logs)
            .expect("registered program should claim instruction");
        assert!(outcome.error.is_some());
        assert_eq!(outcome.resulting_accounts.len(), 0);
    }
}
