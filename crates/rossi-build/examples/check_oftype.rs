use rossi::{parse_predicate_str, pretty::PrettyPrinter};
use rossi_build::normalize::canonical_predicate;

fn main() {
    let input = "sum(∅ ⦂ ℙ(MODULE×ℤ))=0";
    let p = parse_predicate_str(input).unwrap();
    println!("input:  {input}");
    println!("raw:    {}", PrettyPrinter::new().print_predicate(&p));
    println!("canon:  {}", canonical_predicate(&p));
}
