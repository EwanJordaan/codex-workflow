const MIGRATION_DIRECTORIES: [&str; 5] = [
    "goals_migrations",
    "logs_migrations",
    "memory_migrations",
    "migrations",
    "thread_history_migrations",
];

fn main() {
    for directory in MIGRATION_DIRECTORIES {
        println!("cargo:rerun-if-changed={directory}");
        for entry in std::fs::read_dir(directory).unwrap_or_else(|error| {
            panic!("failed to read migration directory {directory}: {error}")
        }) {
            let path = entry
                .unwrap_or_else(|error| panic!("failed to read migration directory entry: {error}"))
                .path();
            if path.extension().is_some_and(|extension| extension == "sql") {
                let contents = std::fs::read(&path).unwrap_or_else(|error| {
                    panic!("failed to read migration {}: {error}", path.display())
                });
                assert!(
                    !contents.contains(&b'\r'),
                    "migration {} must use LF line endings",
                    path.display()
                );
            }
        }
    }
}
