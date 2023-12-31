
use crate::{Token, define::RuleExpression};
use super::{Parser, ParseError, SyntaxTree};

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use by_address::ByAddress;


#[derive(Clone, Debug)]
struct Continuation<'a, T: Token>(usize, Vec<Rc<IntermediateSyntaxTree<'a, T>>>); // usize is the next token to parse

impl<'a, T: Token> PartialEq for Continuation<'a, T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0 && self.1.iter().zip(other.1.iter()).all(|(a, b)| Rc::ptr_eq(a, b))
    }
}

impl<'a, T: Token> PartialOrd for Continuation<'a, T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<'a, T: Token> Eq for Continuation<'a, T> {}
impl<'a, T: Token> Ord for Continuation<'a, T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

pub fn backtracking_parse<T: Token>(parser: &Parser<T>, tokens: &[T], start_rule: &str) -> Result<SyntaxTree<T>, ParseError> {
    let start_expr = RuleExpression::RuleName(start_rule.to_string());

    let mut memo_map: HashMap<(ByAddress<&RuleExpression>, usize), Vec<Continuation<T>>> = HashMap::new();
    let mut failure_info = FailureCache::new();

    parse_expr(parser, tokens, 0, &start_expr, &mut memo_map, &mut failure_info)?;

    if let Some(Continuation (_, trees)) = memo_map[&(ByAddress(&start_expr), 0)].clone().into_iter()
            .find(|Continuation (i, _)| *i == tokens.len()) {
        
        Ok(intermediate_to_final(&trees[0]))
    }
    else if failure_info.index < tokens.len() {
        Err(ParseError::IncompleteParse { 
            index: failure_info.index, 
            terminals: failure_info.failures.into_iter().map(ToString::to_string).collect() 
        })
    }
    else {
        Err(ParseError::OutOfInput { 
            terminals: failure_info.failures.into_iter().map(ToString::to_string).collect() 
        })
    }
    // TODO - also handle ambiguous parse. (?)
}

// Stores failure information to allow creating nice errors.
struct FailureCache<'a> {
    failures: HashSet<&'a str>,
    index: usize,
}

impl<'a> FailureCache<'a> {
    fn new() -> FailureCache<'a> {
        Self { failures: HashSet::new(), index: 0 }
    }

    fn log(&mut self, index: usize, expected: &'a str) {
        if index > self.index {
            self.index = index;
            self.failures.clear();
        }

        if index == self.index {
            self.failures.insert(expected);
        }
    }
}

fn parse_expr<'a, T: Token>(
    parser: &'a Parser<T>, 
    tokens: &[T], 
    token_index: usize, 
    expr: &'a RuleExpression,
    memo_map: &mut HashMap<(ByAddress<&'a RuleExpression>, usize), Vec<Continuation<'a, T>>>,
    failure_info: &mut FailureCache<'a>
) -> Result<(), ParseError> {

    // Prevent stack overflow by allocating additional stack as required.
    stacker::maybe_grow(32 * 1024, 1024 * 1024, || {

        if memo_map.contains_key(&(ByAddress(expr), token_index)) {
            return Ok(());
        }

        let mut continuations = vec![];

        match expr {
            RuleExpression::Terminal(term) => {
                if token_index < tokens.len() && T::matches(term, &tokens[token_index])? {
                    continuations.push(Continuation (
                        token_index + 1,
                        vec![Rc::new(IntermediateSyntaxTree::TokenNode(tokens[token_index].clone()))]
                    ));
                }
                else {
                    failure_info.log(token_index, term);
                }
            },
            RuleExpression::RuleName(rule_name) => {
                match parser.rules.get(rule_name) {
                    Some(rule_expr) => {
                        parse_expr(parser, tokens, token_index, rule_expr, memo_map, failure_info)?;
                        continuations = memo_map[&(ByAddress(rule_expr), token_index)].clone().into_iter()
                            .map(|Continuation (a, subtrees)| 
                                Continuation (a, vec![Rc::new(IntermediateSyntaxTree::RuleNode { rule_name, subexpressions: subtrees })])
                            )
                            .collect();
                    }
                    None => return Err("Rule not found".into()),
                }
            },
            RuleExpression::Concatenation(exprs) => {
                let mut curr_pass = vec![Continuation (token_index, vec![])];

                for expr in exprs {
                    curr_pass = extend_all(curr_pass, parser, tokens, expr, memo_map, failure_info)?;
                }

                continuations = curr_pass.into_iter().collect();
            },
            RuleExpression::Alternatives(exprs) => {
                for expr in exprs {
                    parse_expr(parser, tokens, token_index, expr, memo_map, failure_info)?;

                    continuations.append(&mut memo_map[&(ByAddress(expr), token_index)].clone());
                }
            },
            RuleExpression::Optional(expr) => {
                continuations.push(Continuation (token_index, vec![]));

                parse_expr(parser, tokens, token_index, expr, memo_map, failure_info)?;
                continuations.append(&mut memo_map[&(ByAddress(&**expr), token_index)].clone());
            },
            RuleExpression::Many(inner_expr) | RuleExpression::OneOrMore(inner_expr) => {
                if let RuleExpression::Many(_) = expr {
                    continuations.push(Continuation(token_index, vec![]));
                }

                let mut curr_pass = vec![Continuation (token_index, vec![])];

                while !curr_pass.is_empty() {
                    curr_pass = extend_all(curr_pass, parser, tokens, inner_expr, memo_map, failure_info)?;

                    continuations.append(&mut curr_pass.clone());
                }
            },
        }

        memo_map.insert((ByAddress(expr), token_index), continuations);
        Ok(())
    })
}

// `curr_pass` is a vector of continuations. This function attempts to parse `expr`
// from each of the continuation, generating a new vector of continuations, possibly
// with more or fewer elements.
// Possibly the bottleneck of the algorithm...
fn extend_all<'a, T: Token>(
    curr_pass: Vec<Continuation<'a, T>>,
    parser: &'a Parser<T>, 
    tokens: &[T], 
    expr: &'a RuleExpression,
    memo_map: &mut HashMap<(ByAddress<&'a RuleExpression>, usize), Vec<Continuation<'a, T>>>,
    failure_info: &mut FailureCache<'a>
) -> Result<Vec<Continuation<'a, T>>, ParseError> {

    let mut next_pass = Vec::new();
    for Continuation (index, old_trees) in curr_pass {
        parse_expr(parser, tokens, index, expr, memo_map, failure_info)?;
        next_pass.append(&mut memo_map[&(ByAddress(expr), index)].clone().into_iter()
            .map(|Continuation (i, subtrees)| {
                let mut final_trees = old_trees.clone();
                final_trees.append(&mut subtrees.clone());

                Continuation (i, final_trees)
            })
            .collect()
        );
    }

    Ok(next_pass)
}


#[derive(Clone, Debug)]
enum IntermediateSyntaxTree<'a, T: Token> { // Vec contains Rc's, to be removed later.
    RuleNode {rule_name: &'a str, subexpressions: Vec<Rc<IntermediateSyntaxTree<'a, T>>>},
    TokenNode (T)
}

fn intermediate_to_final<T: Token>(root: &Rc<IntermediateSyntaxTree<T>>) -> SyntaxTree<T> {
    // Prevent stack overflow by allocating additional stack as required.
    stacker::maybe_grow(32 * 1024, 1024 * 1024, || {
        match &*root.clone() {
            IntermediateSyntaxTree::RuleNode {rule_name, subexpressions} => 
                SyntaxTree::RuleNode {
                    rule_name: (*rule_name).to_string(), 
                    subexpressions: subexpressions.iter()
                        .map(|rc_refcell_tree| intermediate_to_final(rc_refcell_tree))
                        .collect()
                },
            IntermediateSyntaxTree::TokenNode(token) => SyntaxTree::TokenNode(token.clone()),
        }
    })
}
