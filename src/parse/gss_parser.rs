
use super::{Token, Parser, SyntaxTree, ParseError};
use crate::define::RuleExpression;
use std::rc::Rc;

use std::cell::RefCell;
use std::collections::HashMap;

use hashable_rc::HashableRc;


pub fn gss_parse_tokens<T: Token>(parser: &Parser<T>, tokens: Vec<T>, start_rule: &str) -> Result<SyntaxTree<T>, ParseError> {
    let root_expr = RuleExpression::RuleName(start_rule.to_string());
    let root_link = Rc::new(GSSLink {
        node: Rc::new(GSSNode {expr: &root_expr, parent: None, parent_data: GSSParentData::NoData}),
        prev: vec![]
    });
    
    /* gss[i] holds all terminals that are set to try to match tokens[i].
     * When the algorithm is finished, the last layer (gss[tokens.len()])
     * holds nodes representing parser processes that have consumed all tokens. */
    let mut gss: Vec<Vec<Rc<GSSLink>>> = vec![
        resolve_to_terminals(Rc::clone(&root_link.node), parser)?.into_iter()
            .map(|node| Rc::new(GSSLink {node, prev: vec![Rc::clone(&root_link)]}))
            .collect()
    ];

    for token in &tokens {
        let mut next_layer = vec![];

        for link in gss.last().ok_or(ParseError("gss uninitialized".to_string()))? {
            next_layer.extend(
                advance_token(&link.node, token, parser)?.into_iter()
                    .map(|node| Rc::new(GSSLink {node: Rc::clone(&node), prev: vec![Rc::clone(link)]}))
                    .collect::<Vec<_>>()
            );
        }

        // TODO: Implement merging.

        gss.push(next_layer);
    }
    
    /* Backtracks from the final node to the first. Final and first are removed, since they are the root rule. 
     * All other nodes correspond to tokens. */
    let backtrace = Parser::<T>::get_backtrace(&gss)?;

    /* Uses the backtrace to determine the hierarchy of rules and tokens, i.e.
     * the final syntax tree */
    Parser::<T>::backtrace_to_tree(backtrace, tokens)
}

#[derive(Clone, Debug)]
enum IntermediateSyntaxTree<T: Token> { // Vec contains Rc's, to be removed later.
    RuleNode {rule_name: String, subexpressions: Vec<Rc<RefCell<IntermediateSyntaxTree<T>>>>},
    TokenNode (T)
}

fn intermediate_to_final<T: Token>(root: Rc<RefCell<IntermediateSyntaxTree<T>>>) -> SyntaxTree<T> {
    match Rc::try_unwrap(root).expect("Last living reference").into_inner() {
        IntermediateSyntaxTree::RuleNode {rule_name, subexpressions} => 
            SyntaxTree::RuleNode {
                rule_name, 
                subexpressions: subexpressions.into_iter()
                    .map(|rc_refcell_tree| intermediate_to_final(rc_refcell_tree))
                    .collect()
            },
        IntermediateSyntaxTree::TokenNode(token) => SyntaxTree::TokenNode(token),
    }
}

impl<T: Token> Parser<T> {
    fn get_backtrace<'a>(gss: &'a [Vec<Rc<GSSLink>>]) -> Result<Vec<Rc<GSSNode<'a>>>, ParseError> {
        let final_links = gss.last()
        .ok_or(ParseError("gss initialized".to_string()))?
        .iter()
        .filter(|link| matches!(link.node.parent_data, GSSParentData::Done))
        .collect::<Vec<_>>();

        let final_link = match final_links.len() {
            0 => Err(ParseError("Unsuccessful Parse...".to_string())),
            1 => {
                Ok(final_links[0])
            },
            _ => Err(ParseError("Ambiguous Parse...".to_string())),
        }?;

        let backtrace = std::iter::successors(Some(final_link), |link|
            match link.prev.len() {
                0 => None,
                _ => Some(&link.prev[0])
            }
        ).map(|link| Rc::clone(&link.node))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();

        let backtrace = backtrace[1..backtrace.len()-1].to_vec();  // Drop ends, they are the root rule. 

        Ok(backtrace)
    }

    fn backtrace_to_tree(backtrace: Vec<Rc<GSSNode<'_>>>, tokens: Vec<T>) -> Result<SyntaxTree<T>, ParseError> {
        /* Examine each token in the backtrace, and ascend through its GSSNode ancestors to
         * identify which rules the token is under. Note that this method means that a rule
         * that parses no tokens is not included at all in the final tree, which might
         * be confusing but should be survivable. */

        let mut subtrees: HashMap<HashableRc::<GSSNode>, Rc<RefCell<IntermediateSyntaxTree<T>>>> = HashMap::new();
        let mut root: Option<Rc<RefCell<IntermediateSyntaxTree<T>>>> = None;

        if backtrace.len() != tokens.len() {
            return Err(ParseError("Backtrace and token stream have different lengths".to_string()));
        }

        for (node, token) in backtrace.into_iter().zip(tokens.into_iter()) {
            if let RuleExpression::Terminal(_) = node.expr {
                let mut curr_node = node;
                let mut curr_subtree = Rc::new(RefCell::new(IntermediateSyntaxTree::TokenNode(token)));

                while let Some(parent) = &curr_node.parent {
                    if let RuleExpression::RuleName(name) = parent.expr {
                        let parent_unprocessed = !subtrees.contains_key(&HashableRc::new(Rc::clone(parent)));

                        if parent_unprocessed {
                            subtrees.insert(HashableRc::new(Rc::clone(parent)), Rc::new(RefCell::new(IntermediateSyntaxTree::RuleNode { rule_name: name.clone(), subexpressions: vec![] })));
                        }
 
                        let parent_tree = subtrees.get(&HashableRc::new(Rc::clone(parent))).expect("Known to contain node");
                        if let IntermediateSyntaxTree::RuleNode { rule_name: _, subexpressions } = &mut *(parent_tree.borrow_mut()) {  // I hate this &mut *(...) thing.
                            subexpressions.push(curr_subtree);
                        }
                        else {
                            panic!("Known to be RuleNode variant");
                        }

                        curr_subtree = Rc::clone(parent_tree);

                        if !parent_unprocessed {
                            break;
                        }
                    } 

                    curr_node = Rc::clone(&curr_node.parent.clone().expect("Known to be Some ()"));

                    if curr_node.parent.is_none() {
                        root = Some(Rc::clone(subtrees.get(&HashableRc::new(Rc::clone(&curr_node))).expect("Known to contain node")));
                    }
                }
            }
            else {
                return Err(ParseError("Non Terminal in backtrace".to_owned()));
            }
        }

        std::mem::drop(subtrees);  // Destroys Rc's allowing intermediate_to_final to take ownership

        /* Final conversion to SyntaxTree. The intermediate tree had to use Rc<RefCell<_>>
         * so that the trees could be shared in the HashMap as well. */
        Ok(intermediate_to_final(
            root.ok_or(ParseError("No root found at end of parsing".to_string()))?
        ))
    }
}

/* Graph Structured Stack! A node models a current position in the parsing process. */
#[derive(PartialEq, Eq)]
// Eq *should* only be needed for use in a HashableRc hashtable, where equality is by reference.
struct GSSNode<'a> {
    expr: &'a RuleExpression,
    parent: Option<Rc<GSSNode<'a>>>, // Corresponds to the parent expression.
    parent_data: GSSParentData // Corresponds to the data regarding this node's relationship to its parent. i.e. which index of the concatenation.
}

// Represents a link between two GSSNodes, where `node` is the current node and `prev` is a node whose continuation
// leads to `node`.
#[derive(Debug)]
struct GSSLink<'a> {
    node: Rc<GSSNode<'a>>,
    prev: Vec<Rc<GSSLink<'a>>>,  // Note: When merging is implemeneted, we will need multiple prev nodes.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GSSParentData {
    Index (usize),
    NoData,
    Done
}

impl std::fmt::Debug for GSSNode<'_> {
    // Required method
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let mut temp = f.debug_struct("GSSNode");
        let res = temp.field("expr", self.expr);

        let res = match self.parent.clone() {
            Some(parent) => res.field("parent.expr", parent.expr),
            None => res.field("parent.expr", &"None"),
        };

        res.field("parent_data", &self.parent_data)
            .finish()
    }
}


/* Returns a set of all next terminal expressions to parse, modelling the next
    * step after consuming a token in a given state. */
fn advance_token<'a, T: Token>(node: &Rc<GSSNode<'a>>, token: &T, parser: &'a Parser<T>) -> Result<Vec<Rc<GSSNode<'a>>>, ParseError> {
    if let GSSParentData::Done = node.parent_data {
        Ok(vec![])
    }
    else {
        match node.expr {
            RuleExpression::Terminal(token_type) if T::matches(token_type, token)? => {
                if let Some(parent) = node.parent.clone() {
                    advance_auto(&parent, parser, node.parent_data)
                }
                else {
                    Err(ParseError("Terminal Expression has no parent".to_string()))
                }
            }
            RuleExpression::Terminal(_) => Ok(vec![]),
            _ => Err(ParseError("Tried to feed token to non terminal expresison".to_string()))
        }
    }
} 

/* In this case, there is no token to consume. */
fn advance_auto<'a, T: Token>(node: &Rc<GSSNode<'a>>, parser: &'a Parser<T>, caller_parent_data: GSSParentData) -> Result<Vec<Rc<GSSNode<'a>>>, ParseError> {
    if caller_parent_data == GSSParentData::Done {
        return Ok(vec![]);
    }

    match node.expr {
        RuleExpression::Terminal(_) => Err(ParseError("Tried to advance terminal without token".to_owned())),
        RuleExpression::RuleName(_) => {
            match node.parent.clone() {
                Some(parent) => advance_auto(&parent, parser, node.parent_data),
                None => Ok(vec![GSSNode {expr: node.expr, parent: None, parent_data: GSSParentData::Done}.into()])
            }
        } 
        RuleExpression::Concatenation(sub_exprs) => {
            if let GSSParentData::Index(i) = caller_parent_data {
                if i+1 >= sub_exprs.len() {
                    advance_auto(
                        &node.parent.clone().ok_or(ParseError("Concatenation without parent".to_owned()))?, 
                        parser,
                        node.parent_data
                    )
                } 
                else {
                    resolve_to_terminals(Rc::new(GSSNode {
                        expr: &sub_exprs[i+1], 
                        parent: Some(Rc::clone(node)),
                        parent_data: GSSParentData::Index(i+1)
                    }), parser)
                }
            }
            else {
                Err(ParseError("Tried to advance Concatenation without index".to_owned()))
            }
        }
        RuleExpression::Alternatives(_) | RuleExpression::Optional(_) => {
            match node.parent.clone() {
                Some(parent) => advance_auto(&parent, parser, node.parent_data),
                None => Err(ParseError("Alternatives or Optional lack parent".to_string()))
            }
        }
        RuleExpression::OneOrMore(sub_expr) | RuleExpression::Many(sub_expr) => {
            match node.parent.clone() {
                Some(parent) => Ok(
                    resolve_to_terminals(Rc::new(GSSNode { 
                        expr: sub_expr, 
                        parent: Some(Rc::clone(node)), 
                        parent_data: GSSParentData::NoData 
                    }), parser)?.into_iter()
                        .chain(advance_auto(&parent, parser, node.parent_data)?.into_iter())
                        .collect::<Vec<_>>()
                ),
                None => Err(ParseError("OneOrMore or Many lack parent".to_string()))
            }
        }
    }
}

/* Recursively substitute while building a GSSTree, until leaves are terminals  */
fn resolve_to_terminals<'a, T: Token>(node: Rc<GSSNode<'a>>, parser: &'a Parser<T>) -> Result<Vec<Rc<GSSNode<'a>>>, ParseError> {
    match node.expr {
        RuleExpression::Terminal(_) => Ok(vec![node]),
        RuleExpression::RuleName(name) => {
            resolve_to_terminals(Rc::new(GSSNode {
                expr: parser.rules.get(name).ok_or(ParseError(format!("Cannot recognize rule {name}")))?, 
                parent: Some(node), 
                parent_data: GSSParentData::NoData
            }), parser)
        }
        RuleExpression::Concatenation(sub_exprs) => {
            resolve_to_terminals(Rc::new(GSSNode {
                expr: &sub_exprs[0],
                parent: Some(node), 
                parent_data: GSSParentData::Index(0),
            }), parser)
        }
        RuleExpression::Alternatives(sub_exprs) => {
            Ok(sub_exprs.iter()
                .map(|expr| 
                    resolve_to_terminals(Rc::new(GSSNode {
                        expr,
                        parent: Some(Rc::clone(&node)),
                        parent_data: GSSParentData::NoData
                    }), parser)
                )
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect()
            )
        },
        RuleExpression::Many(_) => {
            advance_auto(&node, parser, GSSParentData::NoData)
        },
        RuleExpression::Optional(sub_expr) => {
            Ok(
                resolve_to_terminals(Rc::new(GSSNode {
                    expr: sub_expr,
                    parent: Some(Rc::clone(&node)),
                    parent_data: GSSParentData::NoData
                }), parser)?.into_iter()
                    .chain(advance_auto(&node, parser, GSSParentData::NoData)?.into_iter())
                    .collect()
            )
        },
        RuleExpression::OneOrMore(sub_expr) => {
            resolve_to_terminals(Rc::new(GSSNode {
                expr: sub_expr,
                parent: Some(node),
                parent_data: GSSParentData::NoData,
            }), parser)
        }
    }
}