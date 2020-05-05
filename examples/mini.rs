// Mini Rust language
use zub::{ir::*, vm::*};

extern crate logos;
use logos::Logos;

use std::collections::HashMap;


#[derive(Logos, Debug, PartialEq, Clone)]
enum Token<'t> {
    #[regex("[0-9.]+")]
    Number(&'t str),
    #[regex("[a-zA-Z]+")]
    Ident(&'t str),
    #[token("fn")]
    Fun,
    #[token("global")]
    Global,
    #[token("let")]
    Let,
    #[token("if")]
    If,
    #[token("while")]
    While,
    #[token("return")]
    Return,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LCurly,
    #[token("}")]
    RCurly,
    #[token(".")]
    Period,
    #[token(",")]
    Comma,
    #[token(";")]
    Semicolon,
    #[token("+")]
    Add,
    #[token("-")]
    Sub,
    #[token("*")]
    Mul,
    #[token("/")]
    Div,
    #[token("=")]
    Assign,
    #[token("%")]
    Rem,
    #[error]
    #[regex(r"[ \t\n\f]+", logos::skip)]
    Error,
}

#[derive(Debug, Clone)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Op {
    pub fn prec(&self) -> usize {
        use self::Op::*;

        match self {
            Add => 0,
            Sub => 0,
            Mul => 1,
            Div => 1,
            Rem => 1,
        }
    }
}

#[derive(Debug, Clone)]
enum Statement {
    Let(String, Expression, Binding),
    Global(String, Expression),

    Fun(String, Vec<String>, Vec<Statement>, Binding),
    If(Expression, Vec<Statement>, Option<Vec<Statement>>),
    While(Expression, Vec<Statement>),
    Assign(Expression, Expression),
    Return(Option<Expression>),

    Expression(Expression)
}

#[derive(Debug, Clone)]
enum Expression {
    Number(f64),
    Binary(Box<Expression>, Op, Box<Expression>),
    Array(Vec<Expression>),
    Dict(Vec<Expression>, Vec<Expression>), // Don't care about hashmaps :p
    Var(String, Binding), // It will store the proper relative depth
    Call(Box<Expression>, Vec<Expression>),
}

struct Parser<'p> {
    tokens: Vec<Token<'p>>,
    ast: Vec<Statement>,

    top: usize,

    depth_table: HashMap<String, Binding>,
    depth: usize,
    function_depth: usize,
}

impl<'p> Parser<'p> {
    pub fn new(tokens: Vec<Token<'p>>) -> Self {
        Parser {
            tokens,
            ast: Vec::new(),
            top: 0,

            depth_table: HashMap::new(),
            depth: 0,
            function_depth: 0,
        }
    }

    pub fn parse(&mut self) -> Vec<Statement> {
        while self.remaining() > 0 {
            let statement = self.parse_statement();

            if let Some(s) = statement {
                self.ast.push(s)
            }
        }

        self.ast.clone()
    }

    fn parse_statement(&mut self) -> Option<Statement> {
        use self::Token::*;

        match self.current() {
            Global => {
                self.next();

                let name = self.current_slice().unwrap().to_string();

                self.next();

                if self.current() == Assign {
                    self.next();

                    let right = self.parse_expression().unwrap();
                    self.next();

                    self.depth_table.insert(name.clone(), Binding::global(name.as_str()));

                    Some(
                        Statement::Global(
                            name,
                            right,
                        )
                    )
                } else {
                    panic!("Expected `=`")
                }
            },

            Let => {
                self.next();

                let name = self.current_slice().unwrap().to_string();

                self.next();

                if self.current() == Assign {
                    self.next();

                    let right = self.parse_expression().unwrap();
                    self.next();

                    let binding = Binding::local(name.as_str(), self.depth, self.function_depth);
                    self.depth_table.insert(name.clone(), binding.clone());

                    Some(
                        Statement::Let(
                            name,
                            right,
                            binding
                        )
                    )
                } else {
                    panic!("Expected `=`")
                }
            },

            Fun => {
                self.next();
                let name = self.current_slice().unwrap().to_string();

                let binding = Binding::local(name.as_str(), self.depth, self.function_depth);
                self.depth_table.insert(name.clone(), binding.clone());

                self.next();

                if self.current() == LParen {
                    self.next();

                    let mut params = Vec::new();

                    while self.current() != RParen {
                        let name = self.current_slice().unwrap().to_string();
                        params.push(name);

                        self.next();

                        if self.current() == RParen {
                            break
                        }

                        if self.current() != Comma{
                            panic!("Expected `,` in function params, found {:?}", self.current())
                        }

                        self.next()
                    }

                    self.next(); // RParen

                    self.depth += 1;
                    self.function_depth += 1;

                    let body = self.parse_body();

                    self.depth -= 1;
                    self.function_depth -= 1;

                    Some(
                        Statement::Fun(
                            name,
                            params,
                            body,
                            binding
                        )
                    )

                } else {
                    panic!("Expected `(` in function")
                }
            },

            Return => {
                self.next();

                if self.current() == Semicolon {
                    Some(
                        Statement::Return(None)
                    )
                } else {
                    let a = Some(
                        Statement::Return(Some(self.parse_expression().unwrap()))
                    );

                    self.next();

                    a
                }
            }

            Semicolon => {
                self.next();
                None
            }

            c => {
                let a = Some(
                    Statement::Expression(
                        self.parse_expression().unwrap()
                    )
                );

                self.next();

                a
            },
        }
    }

    fn parse_body(&mut self) -> Vec<Statement> {
        use self::Token::*;

        if self.current() != LCurly {
            panic!("Expected `{`")
        }

        self.next();

        let mut body = Vec::new();

        while self.current() != RCurly {
            let statement = self.parse_statement();

            if let Some(s) = statement {
                body.push(s)
            }
        }

        self.next();

        body
    }

    fn parse_expression(&mut self) -> Option<Expression> {
        use self::Token::*;

        let cur = self.current();

        match cur {
            Number(ref n) => {
                Some(
                    Expression::Number(
                        n.clone().parse::<f64>().unwrap()
                    )
                )
            },
            Ident(ref n) => {
                if let Some(depth) = self.depth_table.get(&n.to_string()) {
                    let mut binding = depth.clone();

                    if binding.depth.is_some() {
                        binding.depth = Some(self.depth);
                    }

                    let var = Expression::Var(
                        n.to_string(),
                        binding,
                    );

                    self.next();

                    if self.current() == LParen {
                        self.next();

                        let mut args = Vec::new();

                        while self.current() != RParen {
                            args.push(self.parse_expression().unwrap());
                            self.next();

                            if self.current() == RParen {
                                break
                            }
    
                            if self.current() != Comma{
                                panic!("Expected `,` in call args, found {:?}", self.current())
                            }

                            self.next();
                        }

                        self.next();

                        Some(
                            Expression::Call(
                                Box::new(var),
                                args
                            )
                        )
                    } else {
                        Some(var)
                    }
                } else {
                    panic!("Can't find variable `{}`", n)
                }
            }
            c => { println!("{:?}", c); self.next(); None},
        }
    }

    fn remaining(&self) -> usize {
        if self.top > self.tokens.len() {
            return 0
        }

        self.tokens.len() - self.top
    }

    fn next(&mut self) {
        self.top += 1
    }

    fn current(&self) -> Token {
        self.tokens[self.top.clone()].clone()
    }

    fn current_slice(&self) -> Option<&str> {
        use self::Token::*;

        match self.current() {
            Number(ref s) |
            Ident(ref s) => Some(s),
            _ => None
        }
    }

    fn peek(&self) -> Token {
        self.tokens[self.top + 1].clone()
    }
}

fn codegen_expr(builder: &IrBuilder, expr: &Expression) -> ExprNode {
    use self::Expression::*;

    match expr {
        Number(ref n) => {
            builder.number(*n)
        },

        Var(name, depth) => {
            builder.var(depth.clone())
        },

        Call(ref callee, ref args) => {
            let mut args_ir = Vec::new();

            for arg in args.iter() {
                args_ir.push(codegen_expr(&builder, arg))
            }

            let callee_ir = codegen_expr(&builder, callee);

            builder.call(callee_ir, args_ir, None)
        },

        _ => todo!()
    }
}

fn codegen(builder: &mut IrBuilder, ast: &Vec<Statement>) {
    use self::Statement::*;
    
    for s in ast.iter() {
        match s {
            Let(name, expr, var) => {
                let right = codegen_expr(&builder, expr);
                builder.bind(var.clone(), right)
            },

            Global(name, expr) => {
                let right = codegen_expr(&builder, expr);
                builder.bind(Binding::global(name), right)
            },

            Fun(name, params, body, var) => {
                let params = params.iter().map(|x| x.as_str()).collect::<Vec<&str>>();

                let fun = builder.function(var.clone(), &params.as_slice(), |mut builder| {
                    codegen(&mut builder, body)
                });

                builder.emit(fun);
            },

            Return(ref val) => {
                let value = if let Some(v) = val {
                    Some(
                        codegen_expr(&builder, v)
                    )
                } else {
                    None
                };

                builder.ret(value)
            },

            Expression(ref expr) => {
                let expr = codegen_expr(&builder, expr);
                builder.emit(expr)
            },

            c => todo!("{:#?}", c)
        }
    }
}

const TEST: &'static str = r#"
let a = 10;

fn id() {
    fn bob() {
        return a;
    }
    
    return bob();
}

global foo = id()
"#;

fn main() {
    let mut lex = Token::lexer(TEST);

    let mut parser = Parser::new(lex.collect::<Vec<Token>>());

    let ast = parser.parse();

    let mut builder = IrBuilder::new();
    codegen(&mut builder, &ast);

    let ir = builder.build();

    println!("{:#?}", ir);

    let mut vm = VM::new();
    vm.exec(&ir, true);

    println!("{:#?}", vm.globals)
}