use factory_agent::tools::diff::two_files_patch;

const HEADER: &str = "===================================================================";

#[test]
fn a_change_with_crlf_lines_keeps_the_carriage_returns() {
    let patch = two_files_patch(
        "src/code.ts",
        "src/code.ts",
        "// a\r\nconst a = 1;\r\nconst b = 2;\r\n",
        "// a\r\nconst a = 3;\r\nconst b = 4;\r\n",
    );
    assert_eq!(
        patch,
        format!(
            "Index: src/code.ts\n{HEADER}\n--- src/code.ts\n+++ src/code.ts\n@@ -1,3 +1,3 @@\n // a\r\n-const a = 1;\r\n-const b = 2;\r\n+const a = 3;\r\n+const b = 4;\r\n"
        )
    );
}

#[test]
fn context_is_four_lines_and_a_missing_final_newline_is_marked() {
    let old = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n";
    let new = "l1\nl2\nl3\nl4\nl5\nX\nl7\nl8\nl9\nl10\nl11\nl12";
    assert_eq!(
        two_files_patch("f", "f", old, new),
        format!(
            "Index: f\n{HEADER}\n--- f\n+++ f\n@@ -2,11 +2,11 @@\n l2\n l3\n l4\n l5\n-l6\n+X\n l7\n l8\n l9\n l10\n l11\n-l12\n+l12\n\\ No newline at end of file\n"
        )
    );
}

#[test]
fn identical_files_print_only_the_header_and_an_empty_file_starts_at_zero() {
    assert_eq!(two_files_patch("f", "f", "same\n", "same\n"), format!("Index: f\n{HEADER}\n--- f\n+++ f\n"));
    assert_eq!(two_files_patch("f", "f", "", "new\n"), format!("Index: f\n{HEADER}\n--- f\n+++ f\n@@ -0,0 +1,1 @@\n+new\n"));
}

#[test]
fn distant_changes_become_separate_hunks() {
    let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\n";
    let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nN\no\np\n";
    assert_eq!(
        two_files_patch("f", "f", old, new),
        format!(
            "Index: f\n{HEADER}\n--- f\n+++ f\n@@ -1,6 +1,6 @@\n a\n-b\n+B\n c\n d\n e\n f\n@@ -10,7 +10,7 @@\n j\n k\n l\n m\n-n\n+N\n o\n p\n"
        )
    );
}

#[test]
fn ambiguous_alignments_and_lone_carriage_returns_follow_jsdiff() {
    let cases = [
        ("swap", "a\nb\n", "b\na\n", "@@ -1,2 +1,2 @@\n-a\n b\n+a\n"),
        ("moved block", "a\nb\nc\nd\n", "c\nd\na\nb\n", "@@ -1,4 +1,4 @@\n-a\n-b\n c\n d\n+a\n+b\n"),
        (
            "lone cr",
            "a\rb\rc\r",
            "a\rb\rX\r",
            "@@ -1,1 +1,1 @@\n-a\rb\rc\r\n\\ No newline at end of file\n+a\rb\rX\r\n\\ No newline at end of file\n",
        ),
        ("repeat shrink", "x\nx\nx\n", "x\nx\n", "@@ -1,3 +1,2 @@\n x\n x\n-x\n"),
        ("rotate", "a\nb\nc\n", "c\na\nb\n", "@@ -1,3 +1,3 @@\n+c\n a\n b\n-c\n"),
        ("aba to a", "a\nb\na\n", "a\n", "@@ -1,3 +1,1 @@\n a\n-b\n-a\n"),
        ("a to aba", "a\n", "a\nb\na\n", "@@ -1,1 +1,3 @@\n a\n+b\n+a\n"),
        ("replace middle dup", "a\nb\nb\nc\n", "a\nb\nc\n", "@@ -1,4 +1,3 @@\n a\n b\n-b\n c\n"),
        ("both ends", "x\na\nb\ny\n", "y\na\nb\nx\n", "@@ -1,4 +1,4 @@\n-x\n+y\n a\n b\n-y\n+x\n"),
        ("crlf mixed", "a\r\nb\nc\r\n", "a\r\nB\nc\r\n", "@@ -1,3 +1,3 @@\n a\r\n-b\n+B\n c\r\n"),
        ("empty both", "", "", ""),
        ("old empty new multi", "", "a\nb", "@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n"),
        ("new empty", "a\nb\n", "", "@@ -1,2 +0,0 @@\n-a\n-b\n"),
        ("only newline change", "a\nb", "a\nb\n", "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"),
        ("dup lines swap", "a\na\nb\nb\n", "b\nb\na\na\n", "@@ -1,4 +1,4 @@\n-a\n-a\n b\n b\n+a\n+a\n"),
        ("interleave", "a\nb\nc\nd\ne\nf\n", "a\nc\nb\ne\nd\nf\n", "@@ -1,6 +1,6 @@\n a\n-b\n c\n-d\n+b\n e\n+d\n f\n"),
        ("trailing ws lines", "a \nb\n", "a\nb \n", "@@ -1,2 +1,2 @@\n-a \n-b\n+a\n+b \n"),
        (
            "long ambiguous",
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n",
            "1\n2\n4\n3\n5\n6\n7\n8\n9\n10\n11\n12\n14\n13\n15\n",
            "@@ -1,15 +1,15 @@\n 1\n 2\n-3\n 4\n+3\n 5\n 6\n 7\n 8\n 9\n 10\n 11\n 12\n-13\n 14\n+13\n 15\n",
        ),
        (
            "cr only no newline",
            "a\rb",
            "a\rc",
            "@@ -1,1 +1,1 @@\n-a\rb\n\\ No newline at end of file\n+a\rc\n\\ No newline at end of file\n",
        ),
        ("insert dup", "a\nb\n", "a\na\nb\n", "@@ -1,2 +1,3 @@\n a\n+a\n b\n"),
    ];
    for (name, old, new, body) in cases {
        assert_eq!(two_files_patch("f", "f", old, new), format!("Index: f\n{HEADER}\n--- f\n+++ f\n{body}"), "{name}");
    }
}
