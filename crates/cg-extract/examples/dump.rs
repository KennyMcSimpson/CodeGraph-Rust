// Dev tool: print what the kernel emits for one file.   cargo run -p cg-extract --example dump -- <file> [lang]
use cg_extract::FileRows;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let src = std::fs::read_to_string(path).unwrap();
    let lang = args.get(2).cloned().unwrap_or_else(|| cg_scan::lang::detect_from_path(path, &Default::default()).as_str().to_string());
    let out = cg_kernel::extract_file(path, &src, &lang, &cg_kernel::ExtractOptions::default()).unwrap();
    let rows = FileRows::new(&out);
    println!("# nodes");
    for (i, n) in rows.nodes().enumerate() {
        println!(
            "{i:3} {:<11} {:<24} q={:<30} L{}-{} c{}-{} vis={:?} exp={:?} ret={:?} sig={:?}",
            n.kind().as_str(),
            n.name(),
            n.qname(),
            n.start_line(),
            n.end_line(),
            n.start_col(),
            n.end_col(),
            n.visibility().as_str(),
            n.is_exported(),
            n.return_type(),
            n.signature().map(|s| s.chars().take(50).collect::<String>())
        );
    }
    println!("# edges");
    for e in rows.edges() {
        println!("{:?} -> {:?} {} L{:?} meta={:?}", e.source_row(), e.target_row(), e.kind(), e.line(), e.metadata_json());
    }
    println!("# refs");
    for r in rows.refs() {
        println!(
            "from={:?} {:<12} {:<40} L{}:{} cand={:?}",
            r.from_row(),
            r.kind().as_str(),
            r.name(),
            r.line(),
            r.col(),
            r.candidates().map(|c| c.replace('\0', "|"))
        );
    }
}
