// Fault injection for an owned standalone renderer probe, never a game process.
import Foundation
import Darwin
let args = CommandLine.arguments
guard args.count == 4, let pid = Int32(args[1]), let address = UInt64(args[2], radix:16), let value = UInt8(args[3]), value <= 1 else { exit(64) }
var process = proc_bsdinfo()
let size = Int32(MemoryLayout<proc_bsdinfo>.size)
guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &process, size) == size,
      process.pbi_ppid == UInt32(getppid()) else {
 fputs("refusing a process not owned by the calling test runner\n", stderr); exit(77)
}
var task: mach_port_t = 0
let result = task_for_pid(mach_task_self_,pid,&task)
guard result == KERN_SUCCESS else { fputs("task_for_pid denied; use an entitled disposable test SDK and injector\n", stderr); exit(77) }
defer { mach_port_deallocate(mach_task_self_,task) }
var original: UInt8 = 0
var actual: mach_vm_size_t = 0
let read = withUnsafeMutablePointer(to:&original) { mach_vm_read_overwrite(task,address,1,mach_vm_address_t(UInt(bitPattern:$0)),&actual) }
guard read == KERN_SUCCESS && actual == 1 && original <= 1 else { fatalError("not a readable bool") }
var byte = value
let write = withUnsafeMutablePointer(to:&byte) { mach_vm_write(task,address,vm_offset_t(UInt(bitPattern:$0)),1) }
guard write == KERN_SUCCESS else { fatalError("write failed: \(write)") }
print("original=\(original) injected=\(value)")
