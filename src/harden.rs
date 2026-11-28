use std::io;

pub fn harden() -> io::Result<()> {
    let no_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &no_core) } != 0 {
        return Err(failed("set RLIMIT_CORE to 0"));
    }
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(failed("set PR_SET_DUMPABLE to 0"));
    }
    unsafe { libc::umask(0o077) };
    Ok(())
}

fn failed(step: &str) -> io::Error {
    let cause = io::Error::last_os_error();
    io::Error::new(cause.kind(), format!("{step}: {cause}"))
}
