use std::collections::{hash_map, HashMap};

use jsonata_errors::{Error, Result};
use jsonata_shared::Position;

use super::ast::*;
use super::error::*;
use super::frame::Frame;
use super::functions::*;
use super::value::{ArrayFlags, Value, ValueKind, ValuePool};

pub struct Evaluator {
    pool: ValuePool,
    chain_ast: Ast,
}

impl Evaluator {
    pub fn new(pool: ValuePool, chain_ast: Ast) -> Self {
        Evaluator { pool, chain_ast }
    }

    fn fn_context<'a>(
        &'a self,
        name: &'a str,
        position: &Position,
        input: &'a Value,
        frame: &'a Frame,
    ) -> FunctionContext<'a> {
        FunctionContext {
            name,
            position: *position,
            pool: self.pool.clone(),
            evaluator: self,
            input: input.clone(),
            frame: frame.clone(),
        }
    }

    pub fn evaluate(&self, node: &Ast, input: &Value, frame: &Frame) -> Result<Value> {
        let mut result = match node.kind {
            AstKind::Null => self.pool.null(),
            AstKind::Bool(b) => self.pool.bool(b),
            AstKind::String(ref s) => self.pool.string(String::from(s)),
            AstKind::Number(n) => self.pool.number(n),
            AstKind::Block(ref exprs) => self.evaluate_block(exprs, input, frame)?,
            AstKind::Unary(ref op) => self.evaluate_unary_op(node, op, input, frame)?,
            AstKind::Binary(ref op, ref lhs, ref rhs) => {
                self.evaluate_binary_op(node, op, lhs, rhs, input, frame)?
            }
            AstKind::Var(ref name) => self.evaluate_var(name, input, frame)?,
            AstKind::Ternary {
                ref cond,
                ref truthy,
                ref falsy,
            } => self.evaluate_ternary(cond, truthy, falsy.as_deref(), input, frame)?,
            AstKind::Path(ref steps) => self.evaluate_path(node, steps, input, frame)?,
            AstKind::Name(ref name) => fn_lookup_internal(
                &self.fn_context("lookup", &node.position, input, frame),
                input,
                name,
            ),
            AstKind::Lambda { ref name, .. } => self.pool.lambda(name, node.clone()),
            AstKind::Function {
                ref proc,
                ref args,
                is_partial,
                ..
            } => self.evaluate_function(input, proc, args, is_partial, frame, None)?,

            _ => unimplemented!("TODO: node kind not yet supported: {:#?}", node.kind),
        };

        if let Some(filters) = &node.predicates {
            for filter in filters {
                result = self.evaluate_filter(filter, result, frame)?;
            }
        }

        Ok(if result.has_flags(ArrayFlags::SEQUENCE) {
            if node.keep_array {
                result.add_flags(ArrayFlags::SINGLETON);
            }
            if result.is_empty() {
                self.pool.undefined()
            } else if result.len() == 1 {
                if result.has_flags(ArrayFlags::SINGLETON) {
                    result
                } else {
                    result.get_member(0)
                }
            } else {
                result
            }
        } else {
            result
        })
    }

    fn evaluate_block(&self, exprs: &[Ast], input: &Value, frame: &Frame) -> Result<Value> {
        let frame = Frame::new_with_parent(frame);
        if exprs.is_empty() {
            return Ok(self.pool.undefined());
        }

        let mut result = self.pool.undefined();
        for expr in exprs {
            result = self.evaluate(expr, input, &frame)?;
        }

        Ok(result)
    }

    fn evaluate_var(&self, name: &str, input: &Value, frame: &Frame) -> Result<Value> {
        Ok(if name.is_empty() {
            if input.has_flags(ArrayFlags::WRAPPED) {
                input.get_member(0)
            } else {
                input.clone()
            }
        } else if let Some(value) = frame.lookup(name) {
            value
        } else {
            self.pool.undefined()
        })
    }

    fn evaluate_unary_op(
        &self,
        node: &Ast,
        op: &UnaryOp,
        input: &Value,
        frame: &Frame,
    ) -> Result<Value> {
        match *op {
            UnaryOp::Minus(ref value) => {
                let result = self.evaluate(value, input, frame)?;
                match *result {
                    ValueKind::Undefined => Ok(self.pool.undefined()),
                    ValueKind::Number(num) if !num.is_nan() => Ok(self.pool.number(-num)),
                    _ => Err(d1002_negating_non_numeric(node.position, &result)),
                }
            }
            UnaryOp::ArrayConstructor(ref array) => {
                let mut result = self.pool.array(if node.cons_array {
                    ArrayFlags::CONS
                } else {
                    ArrayFlags::empty()
                });
                for item in array.iter() {
                    let value = self.evaluate(item, input, frame)?;
                    if let AstKind::Unary(UnaryOp::ArrayConstructor(..)) = item.kind {
                        result.push_index(value.index);
                    } else {
                        result = fn_append(
                            &self.fn_context("append", &node.position, input, frame),
                            &result,
                            &value,
                        )?;
                    }
                }
                Ok(result)
            }
            UnaryOp::ObjectConstructor(ref object) => {
                self.evaluate_group_expression(node.position, object, input, frame)
            }
        }
    }

    fn evaluate_group_expression(
        &self,
        position: Position,
        object: &[(Ast, Ast)],
        input: &Value,
        frame: &Frame,
    ) -> Result<Value> {
        struct Group {
            pub data: Value,
            pub index: usize,
        }

        let mut groups: HashMap<String, Group> = HashMap::new();
        let mut input = input.wrap_in_array_if_needed(ArrayFlags::empty());

        if input.is_empty() {
            input.push(ValueKind::Undefined);
        }

        for item in input.members() {
            for (index, pair) in object.iter().enumerate() {
                let key = self.evaluate(&pair.0, &item, frame)?;
                if !key.is_string() {
                    return Err(t1003_non_string_key(position, &key));
                }

                let key = key.as_str();

                match groups.entry(key.to_string()) {
                    hash_map::Entry::Occupied(mut entry) => {
                        let group = entry.get_mut();
                        if group.index != index {
                            return Err(d1009_multiple_keys(position, &key));
                        }
                        group.data = fn_append(
                            &self.fn_context("append", &position, &input, frame),
                            &group.data,
                            &item,
                        )?;
                    }
                    hash_map::Entry::Vacant(entry) => {
                        entry.insert(Group {
                            data: item.clone(),
                            index,
                        });
                    }
                };
            }
        }

        let mut result = self.pool.object();

        for key in groups.keys() {
            let group = groups.get(key).unwrap();
            let value = self.evaluate(&object[group.index].1, &group.data, frame)?;
            if !value.is_undefined() {
                result.insert_index(key, value.index);
            }
        }

        Ok(result)
    }

    fn evaluate_binary_op(
        &self,
        node: &Ast,
        op: &BinaryOp,
        lhs_ast: &Ast,
        rhs_ast: &Ast,
        input: &Value,
        frame: &Frame,
    ) -> Result<Value> {
        if *op == BinaryOp::Bind {
            if let AstKind::Var(ref name) = lhs_ast.kind {
                let rhs = self.evaluate(rhs_ast, input, frame)?;
                frame.bind(name, &rhs);
                return Ok(rhs);
            }
            unreachable!()
        }

        // NOTE: rhs is not evaluated until absolutely necessary to support short circuiting
        // of boolean expressions.
        let lhs = self.evaluate(lhs_ast, input, frame)?;

        match op {
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulus => {
                let rhs = self.evaluate(rhs_ast, input, frame)?;

                let lhs = match *lhs {
                    ValueKind::Undefined => return Ok(self.pool.undefined()),
                    ValueKind::Number(n) if !n.is_nan() => f64::from(n),
                    _ => return Err(t2001_left_side_not_number(node.position, op)),
                };

                let rhs = match *rhs {
                    ValueKind::Undefined => return Ok(self.pool.undefined()),
                    ValueKind::Number(n) if !n.is_nan() => f64::from(n),
                    _ => return Err(t2002_right_side_not_number(node.position, op)),
                };

                let result = match op {
                    BinaryOp::Add => lhs + rhs,
                    BinaryOp::Subtract => lhs - rhs,
                    BinaryOp::Multiply => lhs * rhs,
                    BinaryOp::Divide => lhs / rhs,
                    BinaryOp::Modulus => lhs % rhs,
                    _ => unreachable!(),
                };

                if result.is_infinite() {
                    Err(Error::D1001NumberOfOutRange(result))
                } else {
                    Ok(self.pool.number(result))
                }
            }

            BinaryOp::LessThan
            | BinaryOp::LessThanEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterThanEqual => {
                let rhs = self.evaluate(rhs_ast, input, frame)?;

                if lhs.is_undefined() || rhs.is_undefined() {
                    return Ok(self.pool.undefined());
                }

                if !((lhs.is_number() || lhs.is_string()) && (rhs.is_number() || rhs.is_string())) {
                    return Err(t2010_binary_op_types(node.position, op));
                }

                if let (ValueKind::Number(ref lhs), ValueKind::Number(ref rhs)) = (&*lhs, &*rhs) {
                    let lhs = f64::from(*lhs);
                    let rhs = f64::from(*rhs);
                    return Ok(self.pool.bool(match op {
                        BinaryOp::LessThan => lhs < rhs,
                        BinaryOp::LessThanEqual => lhs <= rhs,
                        BinaryOp::GreaterThan => lhs > rhs,
                        BinaryOp::GreaterThanEqual => lhs >= rhs,
                        _ => unreachable!(),
                    }));
                }

                if let (ValueKind::String(ref lhs), ValueKind::String(ref rhs)) = (&*lhs, &*rhs) {
                    return Ok(self.pool.bool(match op {
                        BinaryOp::LessThan => lhs < rhs,
                        BinaryOp::LessThanEqual => lhs <= rhs,
                        BinaryOp::GreaterThan => lhs > rhs,
                        BinaryOp::GreaterThanEqual => lhs >= rhs,
                        _ => unreachable!(),
                    }));
                }

                Err(t2009_binary_op_mismatch(node.position, &lhs, &rhs, op))
            }

            BinaryOp::Equal | BinaryOp::NotEqual => {
                let rhs = self.evaluate(rhs_ast, input, frame)?;

                if lhs.is_undefined() || rhs.is_undefined() {
                    return Ok(self.pool.bool(false));
                }

                Ok(self.pool.bool(match op {
                    BinaryOp::Equal => lhs == rhs,
                    BinaryOp::NotEqual => lhs != rhs,
                    _ => unreachable!(),
                }))
            }

            BinaryOp::Range => {
                let rhs = self.evaluate(rhs_ast, input, frame)?;

                if !lhs.is_undefined() && !lhs.is_integer() {
                    return Err(Error::T2003LeftSideNotInteger(node.position));
                };

                if !rhs.is_undefined() && !rhs.is_integer() {
                    return Err(Error::T2004RightSideNotInteger(node.position));
                }

                if lhs.is_undefined() || rhs.is_undefined() {
                    return Ok(self.pool.undefined());
                }

                let lhs = lhs.as_usize();
                let rhs = rhs.as_usize();

                if lhs > rhs {
                    return Ok(self.pool.undefined());
                }

                let size = rhs - lhs + 1;
                if size > 10_000_000 {
                    // TODO: D2014
                    unreachable!()
                }

                let mut result = self.pool.array_with_capacity(size, ArrayFlags::SEQUENCE);
                for index in lhs..rhs + 1 {
                    result.push(ValueKind::Number(index.into()));
                }

                Ok(result)
            }

            BinaryOp::Concat => {
                let rhs = self.evaluate(rhs_ast, input, frame)?;
                let mut result = String::new();
                if !lhs.is_undefined() {
                    result.push_str(
                        &fn_string(
                            &self.fn_context("string", &node.position, input, frame),
                            &lhs,
                        )?
                        .as_str(),
                    );
                }
                if !rhs.is_undefined() {
                    result.push_str(
                        &fn_string(
                            &self.fn_context("string", &node.position, input, frame),
                            &rhs,
                        )?
                        .as_str(),
                    );
                }
                Ok(self.pool.string(result))
            }

            BinaryOp::And => Ok(self
                .pool
                .bool(lhs.is_truthy() && self.evaluate(rhs_ast, input, frame)?.is_truthy())),

            BinaryOp::Or => Ok(self
                .pool
                .bool(lhs.is_truthy() || self.evaluate(rhs_ast, input, frame)?.is_truthy())),

            BinaryOp::Apply => {
                if let AstKind::Function {
                    ref proc,
                    ref args,
                    is_partial,
                    ..
                } = rhs_ast.kind
                {
                    // Function invocation with lhs as the first argument
                    self.evaluate_function(input, proc, args, is_partial, frame, Some(&lhs))
                } else {
                    let rhs = self.evaluate(rhs_ast, input, frame)?;

                    if !rhs.is_function() {
                        // TODO T2006
                        unreachable!()
                    }

                    if lhs.is_function() {
                        // Apply function chaining
                        let chain =
                            self.evaluate(&self.chain_ast, &self.pool.undefined(), frame)?;

                        let mut args = self.pool.array_with_capacity(2, ArrayFlags::empty());
                        args.push_index(lhs.index);
                        args.push_index(rhs.index);

                        self.apply_function(
                            lhs_ast.position,
                            &self.pool.undefined(),
                            &chain,
                            &args,
                            frame,
                        )
                    } else {
                        let mut args = self.pool.array_with_capacity(1, ArrayFlags::empty());
                        args.push_index(lhs.index);
                        self.apply_function(
                            rhs_ast.position,
                            &self.pool.undefined(),
                            &rhs,
                            &args,
                            frame,
                        )
                    }
                }
            }

            BinaryOp::In => {
                let rhs = self.evaluate(rhs_ast, input, frame)?;

                if lhs.is_undefined() || rhs.is_undefined() {
                    return Ok(self.pool.bool(false));
                }

                let rhs = rhs.wrap_in_array_if_needed(ArrayFlags::empty());

                for item in rhs.members() {
                    if item == lhs {
                        return Ok(self.pool.bool(true));
                    }
                }

                Ok(self.pool.bool(false))
            }

            _ => unimplemented!("TODO: binary op not supported yet: {:#?}", *op),
        }
    }

    fn evaluate_ternary(
        &self,
        cond: &Ast,
        truthy: &Ast,
        falsy: Option<&Ast>,
        input: &Value,
        frame: &Frame,
    ) -> Result<Value> {
        let cond = self.evaluate(cond, input, frame)?;
        if cond.is_truthy() {
            self.evaluate(truthy, input, frame)
        } else if let Some(falsy) = falsy {
            self.evaluate(falsy, input, frame)
        } else {
            Ok(self.pool.undefined())
        }
    }

    fn evaluate_path(
        &self,
        node: &Ast,
        steps: &[Ast],
        input: &Value,
        frame: &Frame,
    ) -> Result<Value> {
        let mut input = if input.is_array() && !matches!(steps[0].kind, AstKind::Var(..)) {
            input.clone()
        } else {
            input.wrap_in_array(ArrayFlags::empty())
        };

        let mut result = self.pool.undefined();

        for (index, step) in steps.iter().enumerate() {
            result = if index == 0 && step.cons_array {
                self.evaluate(step, &input, frame)?
            } else {
                self.evaluate_step(step, &input, frame, index == steps.len() - 1)?
            };

            if result.is_undefined() || (result.is_array() && result.is_empty()) {
                break;
            }

            // if step.focus.is_none() {
            input = result.clone();
            // }
        }

        if node.keep_singleton_array {
            let mut flags = result.get_flags();
            if flags.contains(ArrayFlags::CONS) && !flags.contains(ArrayFlags::SEQUENCE) {
                result = result.wrap_in_array(flags | ArrayFlags::SEQUENCE);
            }
            flags |= ArrayFlags::SINGLETON;
            result.set_flags(flags);
        }

        if let Some((position, ref object)) = node.group_by {
            result = self.evaluate_group_expression(position, object, &result, frame)?;
        }

        Ok(result)
    }

    fn evaluate_step(
        &self,
        step: &Ast,
        input: &Value,
        frame: &Frame,
        last_step: bool,
    ) -> Result<Value> {
        let mut result = self.pool.array(ArrayFlags::SEQUENCE);

        if let AstKind::Sort(ref sorts) = step.kind {
            result = self.evaluate_sorts(sorts, input, frame)?;
            if let Some(ref stages) = step.stages {
                result = self.evaluate_stages(stages, result, frame)?;
            }
            return Ok(result);
        }

        for item in input.members() {
            let mut item_result = self.evaluate(step, &item, frame)?;

            if let Some(ref stages) = step.stages {
                for stage in stages {
                    item_result = self.evaluate_filter(stage, item_result, frame)?;
                }
            }

            if !item_result.is_undefined() {
                result.push_index(item_result.index);
            }
        }

        Ok(
            if last_step
                && result.len() == 1
                && result.get_member(0).is_array()
                && !result.get_member(0).has_flags(ArrayFlags::SEQUENCE)
            {
                result.get_member(0)
            } else {
                let mut result_sequence = self.pool.array(ArrayFlags::SEQUENCE);

                for result_item in result.members() {
                    if !result_item.is_array() || result_item.has_flags(ArrayFlags::CONS) {
                        result_sequence.push_index(result_item.index);
                    } else {
                        for item in result_item.members() {
                            result_sequence.push_index(item.index);
                        }
                    }
                }
                result_sequence
            },
        )
    }

    fn evaluate_sorts(
        &self,
        _sorts: &[(Ast, bool)],
        _inputt: &Value,
        _frame: &Frame,
    ) -> Result<Value> {
        unimplemented!("Sorts not yet implemented")
    }

    fn evaluate_stages(&self, _stages: &[Ast], _input: Value, _frame: &Frame) -> Result<Value> {
        unimplemented!("Stages not yet implemented")
    }

    fn evaluate_filter(&self, node: &Ast, input: Value, frame: &Frame) -> Result<Value> {
        let mut result = self.pool.array(ArrayFlags::SEQUENCE);
        let input = input.wrap_in_array_if_needed(ArrayFlags::empty());

        let get_index = |n: f64| {
            let mut index = n.floor() as isize;
            let length = if input.is_array() {
                input.len() as isize
            } else {
                1
            };
            if index < 0 {
                // Count from the end of the array
                index += length;
            }
            index as usize
        };

        match node.kind {
            AstKind::Filter(ref filter) => match filter.kind {
                AstKind::Number(n) => {
                    let index = get_index(n.into());
                    let item = input.get_member(index as usize);
                    if !item.is_undefined() {
                        if item.is_array() {
                            result = item;
                        } else {
                            result.push_index(item.index);
                        }
                    }
                }
                _ => {
                    for (i, item) in input.members().enumerate() {
                        let mut index = self.evaluate(filter, &item, frame)?;
                        if index.is_number() && !index.is_nan() {
                            index = index.wrap_in_array(ArrayFlags::empty());
                        }
                        if index.is_array() && index.members().all(|v| v.is_number() && !v.is_nan())
                        {
                            index.members().for_each(|v| {
                                let index = get_index(v.as_f64());
                                if index == i {
                                    result.push_index(item.index);
                                }
                            });
                        } else if index.is_truthy() {
                            result.push_index(item.index);
                        }
                    }
                }
            },
            _ => unimplemented!("Filters other than numbers are not yet supported"),
        };

        Ok(result)
    }

    pub fn evaluate_function(
        &self,
        input: &Value,
        proc: &Ast,
        args: &[Ast],
        _is_partial: bool,
        frame: &Frame,
        context: Option<&Value>,
    ) -> Result<Value> {
        let evaluated_proc = self.evaluate(proc, input, frame)?;

        // Help the user out if they forgot a '$'
        if evaluated_proc.is_undefined() {
            if let AstKind::Path(ref steps) = proc.kind {
                if let AstKind::Name(ref name) = steps[0].kind {
                    if frame.lookup(name).is_some() {
                        return Err(Error::T1005InvokedNonFunctionSuggest(
                            proc.position,
                            name.clone(),
                        ));
                    }
                }
            }
        }

        let mut evaluated_args = self.pool.array(ArrayFlags::empty());

        if let Some(context) = context {
            evaluated_args.push_index(context.index);
        }

        for arg in args {
            let arg = self.evaluate(arg, input, frame)?;
            evaluated_args.push_index(arg.index);
        }

        self.apply_function(
            proc.position,
            input,
            &evaluated_proc,
            &evaluated_args,
            frame,
        )
    }

    pub fn apply_function(
        &self,
        position: Position,
        input: &Value,
        evaluated_proc: &Value,
        evaluated_args: &Value,
        frame: &Frame,
    ) -> Result<Value> {
        match **evaluated_proc {
            ValueKind::Lambda(_, ref lambda) => {
                if let AstKind::Lambda {
                    ref body, ref args, ..
                } = lambda.kind
                {
                    // Create a new frame for use in the lambda, so it can have locals
                    let frame = Frame::new_with_parent(frame);

                    // Bind the arguments to their respective names
                    for (index, arg) in args.iter().enumerate() {
                        if let AstKind::Var(ref name) = arg.kind {
                            frame.bind(name, &evaluated_args.get_member(index));
                        } else {
                            unreachable!()
                        }
                    }

                    // Evaluate the lambda!
                    self.evaluate(body, input, &frame)
                } else {
                    unreachable!()
                }
            }
            ValueKind::NativeFn0(ref name, ref func) => {
                func(&self.fn_context(name, &position, input, frame))
            }
            ValueKind::NativeFn1(ref name, ref func) => {
                let context = self.fn_context(name, &position, input, frame);
                if evaluated_args.len() > 1 {
                    Err(t0410_argument_not_valid(&context, 2))
                } else if evaluated_args.is_empty() {
                    // Some functions take the input as the first argument if one was not provided
                    func(&context, input)
                } else {
                    func(&context, &evaluated_args.get_member(0))
                }
            }
            ValueKind::NativeFn2(ref name, ref func) => {
                let context = self.fn_context(name, &position, input, frame);
                if evaluated_args.len() > 2 {
                    Err(t0410_argument_not_valid(&context, 3))
                } else {
                    func(
                        &context,
                        &evaluated_args.get_member(0),
                        &evaluated_args.get_member(1),
                    )
                }
            }
            ValueKind::NativeFn3(ref name, ref func) => {
                let context = self.fn_context(name, &position, input, frame);
                if evaluated_args.len() > 3 {
                    Err(t0410_argument_not_valid(&context, 4))
                } else {
                    func(
                        &context,
                        &evaluated_args.get_member(0),
                        &evaluated_args.get_member(1),
                        &evaluated_args.get_member(2),
                    )
                }
            }
            _ => Err(Error::T1006InvokedNonFunction(position)),
        }
    }
}
