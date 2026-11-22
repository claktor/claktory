//! Shared poison-recovering locks for state that stays usable after a panic.

/// Locks a mutex, keeping its contents if a previous owner panicked.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
