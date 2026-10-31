// Dev tool: print the tree-sitter tree of a file.   cargo run -p cg-extract --example sexp -- <file> [lang]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = std::fs::read_to_string(&args[1]).unwrap();
    let lang = args.get(2).cloned().unwrap_or_else(|| cg_scan::lang::detect_from_path(&args[1], &Default::default()).as_str().to_string());
    let mut p = tree_sitter::Parser::new();
    p.set_language(&cg_kernel::hooks::language(&lang).unwrap()).unwrap();
    let t = p.parse(&src, None).unwrap();
    fn show(n: tree_sitter::Node, src: &str, depth: usize, field: Option<&str>) {
        if !n.is_named() { return; }
        let text: String = src[n.start_byte()..n.end_byte()].chars().take(40).collect::<String>().replace('\n', "\\n");
        println!("{}{}{} [{}]", "  ".repeat(depth), field.map(|f| format!("{f}: ")).unwrap_or_default(), n.kind(), if n.child_count() == 0 { text } else { String::new() });
        let mut c = n.walk();
        for (i, ch) in n.children(&mut c).enumerate() {
            show(ch, src, depth + 1, n.field_name_for_child(i as u32));
        }
    }
    show(t.root_node(), &src, 0, None);
}
