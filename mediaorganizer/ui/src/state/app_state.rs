//! Global application state: loaded duplicate pairs and UI selection.

#[cfg(feature = "server")]
use app_core::db::{DuplicatePair, FileRecord};
#[cfg(not(feature = "server"))]
pub mod stubs {
    #[derive(Debug, Clone, Default)] pub struct DuplicatePair {
        pub file_a: String, pub file_b: String, pub similarity: f32,
        pub clip_offset_secs: Option<f64>,
        pub method_str: String,
    }
    #[derive(Debug, Clone, Default)] pub struct FileRecord {
        pub id: String,
        pub path: camino::Utf8PathBuf,
        pub name: String,
        pub size_bytes: u64,
    }
    impl FileRecord {
        pub fn duration_secs(&self) -> f64 { 0.0 }
        pub fn width(&self) -> Option<u32> { None }
        pub fn height(&self) -> Option<u32> { None }
    }
}
#[cfg(not(feature = "server"))]
use stubs::{DuplicatePair, FileRecord};

/// A duplicate cluster: a group of files all connected by duplicate_of edges.
///
/// Built by traversing the SurrealDB graph: walk from every file following
/// ->duplicate_of edges to collect transitive duplicates into one cluster.
#[derive(Debug, Clone)]
pub struct DuplicateCluster {
    /// All files in this cluster (at least 2).
    pub files: Vec<FileRecord>,
    /// The edges that connect them, carrying match evidence.
    pub edges: Vec<DuplicatePair>,
    /// Highest similarity score among all edges in the cluster.
    pub max_similarity: f32,
}

/// Global application state provided at the App root.
///
/// Provided via `use_context_provider(|| Signal::new(AppState::default()))`.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    /// All duplicate clusters loaded from SurrealDB after a scan completes.
    pub clusters: Vec<DuplicateCluster>,
    /// File IDs of the pair currently open in CompareView.
    pub selected_pair: Option<(String, String)>,
    /// Sort order for the results list.
    pub sort: ResultSort,
    /// Filter: only show clusters matching this method.
    pub method_filter: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResultSort {
    #[default]
    SimilarityDesc,
    SimilarityAsc,
    SizeDesc,
}

impl AppState {
    pub fn load_clusters(&mut self, pairs: Vec<DuplicatePair>, files: Vec<FileRecord>) {
        // Build a file lookup map: id → FileRecord
        let file_map: std::collections::HashMap<String, FileRecord> =
            files.into_iter().map(|f| (f.id.clone(), f)).collect();

        // Union-find to group files into clusters
        let mut parent: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        fn find(parent: &mut std::collections::HashMap<String, String>, x: &str) -> String {
            if parent.get(x).map(|p| p == x).unwrap_or(true) {
                parent.insert(x.to_string(), x.to_string());
                return x.to_string();
            }
            let p = parent[x].clone();
            let root = find(parent, &p);
            parent.insert(x.to_string(), root.clone());
            root
        }

        for pair in &pairs {
            let ra = find(&mut parent, &pair.file_a);
            let rb = find(&mut parent, &pair.file_b);
            if ra != rb {
                parent.insert(rb, ra);
            }
        }

        // Group pairs by cluster root
        let mut cluster_map: std::collections::HashMap<String, Vec<DuplicatePair>> =
            std::collections::HashMap::new();
        for pair in &pairs {
            let root = find(&mut parent, &pair.file_a);
            cluster_map.entry(root).or_default().push(pair.clone());
        }

        self.clusters = cluster_map
            .into_values()
            .map(|edges| {
                let mut file_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
                for e in &edges {
                    file_ids.insert(e.file_a.clone());
                    file_ids.insert(e.file_b.clone());
                }
                let files: Vec<FileRecord> = file_ids
                    .iter()
                    .filter_map(|id| file_map.get(id).cloned())
                    .collect();
                let max_similarity = edges
                    .iter()
                    .map(|e| e.similarity)
                    .fold(0f32, f32::max);
                DuplicateCluster { files, edges, max_similarity }
            })
            .collect();

        // Default sort: highest similarity first
        self.clusters.sort_by(|a, b| {
            b.max_similarity.partial_cmp(&a.max_similarity).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Remove a single file from whichever cluster contains it.
    /// If the cluster drops below 2 files it is removed entirely.
    pub fn remove_file(&mut self, file_id: &str) {
        self.clusters.retain_mut(|cluster| {
            cluster.files.retain(|f| f.id != file_id);
            cluster.edges.retain(|e| e.file_a != file_id && e.file_b != file_id);
            if cluster.files.len() >= 2 {
                cluster.max_similarity = cluster
                    .edges
                    .iter()
                    .map(|e| e.similarity)
                    .fold(0f32, f32::max);
                true
            } else {
                false
            }
        });
    }

    /// Remove the cluster that contains the given set of file IDs.
    pub fn remove_cluster_containing(&mut self, file_ids: &[String]) {
        self.clusters.retain(|cluster| {
            !cluster.files.iter().any(|f| file_ids.contains(&f.id))
        });
    }
}
