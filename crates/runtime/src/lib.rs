//! Chronicle GM 运行时：AI 任务调度中心、模型进程托管、资源监控。

pub mod advice;
pub mod client;
pub mod estimate;
pub mod monitor;
pub mod procs;
pub mod sched;

pub use procs::{ModelManager, ModelProfile, ModelRole, ProcState, ProcStatus};
pub use client::{BeforeRequest, ScheduledClient, service_key};
pub use sched::{PerfMode, Permit, Policy, Priority, SchedError, SchedSnapshot, Scheduler, TaskInfo, TaskState, Ticket};
pub use estimate::{MemEstimate, estimate_llama};
pub use monitor::{GpuSample, ModelMem, Monitor, ProcUsage, ResourceSample};
pub use advice::{MemoryAdvice, memory_advice};
