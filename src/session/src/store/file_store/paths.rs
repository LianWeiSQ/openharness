impl FileSessionStore {
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(session_id)
    }

    fn run_dir(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.session_dir(session_id).join("runs").join(run_id)
    }

    fn session_json_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("session.json")
    }

    fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("transcript.jsonl")
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("state.latest.json")
    }

    fn run_json_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("run.json")
    }

    fn events_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("events.jsonl")
    }

    fn parts_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("parts.jsonl")
    }

    fn summary_path(&self, session_id: &str, run_id: &str) -> PathBuf {
        self.run_dir(session_id, run_id).join("summary.json")
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.jsonl")
    }
}
