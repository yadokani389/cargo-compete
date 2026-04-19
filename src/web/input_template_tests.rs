use super::*;

#[test]
fn parse_varlen_rows_with_fixed_prefixes() {
    let lines = vec![
        "P_1 C_1 F_{1,1} F_{1,2} \\ldots F_{1,C_1}".to_string(),
        "P_2 C_2 F_{2,1} F_{2,2} \\ldots F_{2,C_2}".to_string(),
        "\\vdots".to_string(),
        "P_N C_N F_{N,1} F_{N,2} \\ldots F_{N,C_N}".to_string(),
    ];
    let got = parse_varlen_rows(&lines, 0).expect("must parse");
    assert_eq!(got.0, vec!["P".to_string(), "C".to_string()]);
    assert_eq!(got.1, "c");
    assert_eq!(got.2, "F");
    assert_eq!(got.3, "n");
}

#[test]
fn guess_input_handles_strictly_superior_format() {
    let lines = vec![
        "N M".to_string(),
        "P_1 C_1 F_{1,1} F_{1,2} \\ldots F_{1,C_1}".to_string(),
        "\\vdots".to_string(),
        "P_N C_N F_{N,1} F_{N,2} \\ldots F_{N,C_N}".to_string(),
    ];
    let signed = std::collections::HashSet::new();
    let got = guess_input_from_lines(&lines, &signed);
    assert!(got.decls.iter().all(|d| !d.contains("TODO")));
    assert!(got.decls.iter().any(|d| d.contains("n: usize")));
    assert!(got.decls.iter().any(|d| d.contains("m: usize")));
    assert!(got
        .extra_lines
        .iter()
        .any(|l| l.contains("let mut p: Vec<usize>")));
    assert!(got
        .extra_lines
        .iter()
        .any(|l| l.contains("let mut c: Vec<usize>")));
    assert!(got
        .extra_lines
        .iter()
        .any(|l| l.contains("let mut f: Vec<Vec<usize>>")));
    assert!(got
        .extra_lines
        .iter()
        .any(|l| l.contains("input! { p_i: usize, c_i: usize, f_row: [usize; c_i] };")));
}

#[test]
fn render_section_handles_strictly_superior_block() {
    let task = TaskSection {
        letter: "D".to_string(),
        input_blocks: vec![vec![
            "N M".to_string(),
            "P _ 1 C _ 1 F _ {1,1} F _ {1,2} \\ldots F _ {1,C _ 1}".to_string(),
            "\\vdots".to_string(),
            "P _ N C _ N F _ {N,1} F _ {N,2} \\ldots F _ {N,C _ N}".to_string(),
        ]],
        constraints_items: vec![],
    };
    let rendered = render_section(&task).expect("render succeeds");
    assert!(!rendered.contains("TODO"));
    assert!(rendered.contains("let mut p: Vec<usize>"), "{}", rendered);
    assert!(rendered.contains("let mut c: Vec<usize>"));
    assert!(rendered.contains("let mut f: Vec<Vec<usize>>"));
    assert!(rendered.contains("use proconio::input;\n\nfn main() {"));
    assert!(rendered.contains("input! { p_i: usize, c_i: usize, f_row: [usize; c_i] };"));
}

#[test]
fn render_section_handles_e_manga() {
    let task = TaskSection {
        letter: "E".to_string(),
        input_blocks: vec![vec!["N".to_string(), "a_1 \\ldots a_N".to_string()]],
        constraints_items: vec![
            "1 <= N <= 3 * 10^5".to_string(),
            "1 <= a_i <= 10^9".to_string(),
        ],
    };
    let rendered = render_section(&task).expect("render succeeds");
    assert!(!rendered.contains("TODO"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("a: [usize; n]"), "{}", rendered);
}

#[test]
fn render_section_handles_f_ladder() {
    let task = TaskSection {
        letter: "F".to_string(),
        input_blocks: vec![vec![
            "N".to_string(),
            "A_1 B_1".to_string(),
            "A_2 B_2".to_string(),
            "\\ldots".to_string(),
            "A_N B_N".to_string(),
        ]],
        constraints_items: vec![
            "1 <= N <= 2 * 10^5".to_string(),
            "1 <= A_i, B_i <= 10^9".to_string(),
        ],
    };
    let rendered = render_section(&task).expect("render succeeds");
    assert!(!rendered.contains("TODO"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("ab: [(usize, usize); n]"), "{}", rendered);
}

#[test]
fn render_section_handles_g_gravity() {
    let task = TaskSection {
        letter: "G".to_string(),
        input_blocks: vec![vec![
            "N W".to_string(),
            "X_1 Y_1".to_string(),
            "X_2 Y_2".to_string(),
            "\\vdots".to_string(),
            "X_N Y_N".to_string(),
            "Q".to_string(),
            "T_1 A_1".to_string(),
            "T_2 A_2".to_string(),
            "\\vdots".to_string(),
            "T_Q A_Q".to_string(),
        ]],
        constraints_items: vec![
            "1 <= N <= 2 * 10^5".to_string(),
            "1 <= W <= N".to_string(),
            "1 <= X_i <= W".to_string(),
            "1 <= Y_i <= 10^9".to_string(),
            "1 <= Q <= 2 * 10^5".to_string(),
            "1 <= T_j <= 10^9".to_string(),
            "1 <= A_j <= N".to_string(),
        ],
    };
    let rendered = render_section(&task).expect("render succeeds");
    assert!(!rendered.contains("TODO"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("w: usize"), "{}", rendered);
    assert!(rendered.contains("xy: [(usize, usize); n]"), "{}", rendered);
    assert!(rendered.contains("q: usize"), "{}", rendered);
    assert!(rendered.contains("ta: [(usize, usize); q]"), "{}", rendered);
}
