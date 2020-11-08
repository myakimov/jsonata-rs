use json::{array, JsonValue};
use std::ops::{Index, RangeBounds};
use std::slice::Iter;
use std::vec::Drain;

use crate::ast::*;
use crate::error::*;
use crate::frame::{Binding, Frame};
use crate::functions::*;
use crate::JsonAtaResult;

#[derive(Clone, Debug)]
pub enum Value {
    Undefined,
    Raw(JsonValue),
    Array {
        arr: Vec<Value>,
        is_seq: bool,
        keep_array: bool,
    },
}

impl Value {
    pub fn new(raw: Option<&JsonValue>) -> Self {
        match raw {
            None => Self::Undefined,
            Some(raw) => match raw {
                JsonValue::Array(arr) => Self::Array {
                    arr: arr.iter().map(|v| Self::new(Some(v))).collect(),
                    is_seq: false,
                    keep_array: false,
                },
                _ => Self::Raw(raw.clone()),
            },
        }
    }

    pub fn new_array() -> Self {
        Self::Array {
            arr: vec![],
            is_seq: false,
            keep_array: false,
        }
    }

    pub fn new_seq() -> Self {
        Self::Array {
            arr: vec![],
            is_seq: true,
            keep_array: false,
        }
    }

    pub fn new_seq_from(value: &Value) -> Self {
        Self::Array {
            arr: vec![value.clone()],
            is_seq: true,
            keep_array: false,
        }
    }

    pub fn is_undef(&self) -> bool {
        match self {
            Value::Undefined => true,
            _ => false,
        }
    }

    pub fn is_raw(&self) -> bool {
        match self {
            Value::Raw(..) => true,
            _ => false,
        }
    }

    pub fn is_array(&self) -> bool {
        match self {
            Value::Array { .. } => true,
            _ => false,
        }
    }

    pub fn is_seq(&self) -> bool {
        match self {
            Value::Array { is_seq, .. } => *is_seq,
            _ => false,
        }
    }

    pub fn as_raw(&self) -> &JsonValue {
        match self {
            Value::Raw(value) => &value,
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn as_array_mut(&mut self) -> &mut Vec<Value> {
        match self {
            Value::Array { arr, .. } => arr,
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn keep_array(&self) -> bool {
        match self {
            Value::Array { keep_array, .. } => *keep_array,
            _ => false,
        }
    }

    pub fn set_keep_array(&mut self) {
        match self {
            Value::Array { keep_array, .. } => *keep_array = true,
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Value::Array { arr, .. } => arr.is_empty(),
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Value::Array { arr, .. } => arr.len(),
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn iter(&self) -> Iter<'_, Value> {
        match self {
            Value::Array { arr, .. } => arr.iter(),
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn push(&mut self, value: Value) {
        match self {
            Value::Array { arr, .. } => arr.push(value),
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn drain<R>(&mut self, range: R) -> Drain<'_, Value>
    where
        R: RangeBounds<usize>,
    {
        match self {
            Value::Array { arr, .. } => arr.drain(range),
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn take(&mut self) -> JsonValue {
        match self {
            Value::Raw(raw) => raw.take(),
            _ => panic!("unexpected Value type"),
        }
    }

    pub fn to_json(mut self) -> Option<JsonValue> {
        match self {
            Value::Undefined => None,
            Value::Raw(raw) => Some(raw),
            Value::Array { .. } => Some(JsonValue::Array(
                self.drain(..)
                    .filter(|v| !v.is_undef())
                    .map(|v| v.to_json().unwrap())
                    .collect(),
            )),
        }
    }
}

impl Index<usize> for Value {
    type Output = Value;

    fn index(&self, index: usize) -> &Self::Output {
        match self {
            Value::Array { arr, .. } => &arr[index],
            _ => panic!("unexpected Value type"),
        }
    }
}

impl From<Value> for Option<JsonValue> {
    fn from(value: Value) -> Self {
        value.to_json()
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        if self.is_undef() && other.is_undef() {
            true
        } else if self.is_raw() && other.is_raw() {
            self.as_raw() == other.as_raw()
        } else if self.is_array() && other.is_array() {
            if self.len() != other.len() {
                false
            } else {
                for i in 0..self.len() - 1 {
                    if self[i] != other[i] {
                        return false;
                    }
                }
                true
            }
        } else {
            false
        }
    }
}

pub fn evaluate(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    let mut result = match &node.kind {
        NodeKind::Null => Value::Raw(JsonValue::Null),
        NodeKind::Bool(value) => Value::Raw(json::from(*value)),
        NodeKind::Str(value) => Value::Raw(json::from(value.clone())),
        NodeKind::Num(value) => Value::Raw(json::from(*value)),
        NodeKind::Name(_) => evaluate_name(node, input)?,
        NodeKind::Unary(_) => evaluate_unary_op(node, input, frame)?,
        NodeKind::Binary(_) => evaluate_binary_op(node, input, frame)?,
        NodeKind::Block => evaluate_block(node, input, frame)?,
        NodeKind::Ternary => evaluate_ternary(node, input, frame)?,
        NodeKind::Var(name) => evaluate_variable(name, frame)?,
        NodeKind::Path => evaluate_path(node, input, frame)?,
        _ => unimplemented!("TODO: node kind not yet supported: {}", node.kind),
    };

    // TODO: Predicate and grouping (jsonata.js:127)

    if result.is_seq() {
        if result.len() == 0 {
            Ok(Value::Undefined)
        } else if result.len() == 1 {
            Ok(result.as_array_mut().swap_remove(0))
        } else {
            Ok(result)
        }
    } else {
        Ok(result)
    }
}

fn evaluate_name(node: &Node, input: &Value) -> JsonAtaResult<Value> {
    if let NodeKind::Name(key) = &node.kind {
        Ok(lookup(input, key))
    } else {
        unreachable!()
    }
}

fn evaluate_unary_op(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    if let NodeKind::Unary(op) = &node.kind {
        match op {
            UnaryOp::Minus => {
                let result = evaluate(&node.children[0], input, frame)?;
                match result {
                    Value::Raw(raw) => {
                        if let Some(raw) = raw.as_f64() {
                            Ok(Value::Raw((-raw).into()))
                        } else {
                            Err(Box::new(D1002 {
                                position: node.position,
                                value: raw.to_string(),
                            }))
                        }
                    }
                    _ => panic!("`result` should've been an Input::Value"),
                }
            }
            UnaryOp::Array => {
                let mut result = Value::new_array();
                for child in &node.children {
                    let value = evaluate(child, input, frame)?;
                    if !value.is_undef() {
                        if let NodeKind::Unary(UnaryOp::Array) = child.kind {
                            result.push(value)
                        } else {
                            result = append(result, value);
                        }
                    }
                }
                if node.keep_array {
                    result.set_keep_array();
                }
                Ok(result)
            }
            UnaryOp::Object => unimplemented!("TODO: object constructors not yet supported"),
        }
    } else {
        panic!("`node` should be a NodeKind::Unary");
    }
}

fn evaluate_binary_op(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    use BinaryOp::*;
    if let NodeKind::Binary(op) = &node.kind {
        match op {
            Add | Subtract | Multiply | Divide | Modulus => {
                evaluate_numeric_expression(node, input, frame, op)
            }
            LessThan | LessThanEqual | GreaterThan | GreaterThanEqual => {
                evaluate_comparison_expression(node, input, frame, op)
            }
            Equal | NotEqual => evaluate_equality_expression(node, input, frame, op),
            Concat => evaluate_string_concat(node, input, frame),
            Bind => evaluate_bind_expression(node, input, frame),
            Or | And => evaluate_boolean_expression(node, input, frame, op),
            In => evaluate_includes_expression(node, input, frame),
            _ => unimplemented!("TODO: Binary op {:?} not yet supported", op),
        }
    } else {
        panic!("`node` should be a NodeKind::Binary")
    }
}

fn evaluate_bind_expression(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    let name = &node.children[0];
    let value = evaluate(&node.children[1], input, frame)?;

    if !value.is_undef() {
        if let NodeKind::Var(name) = &name.kind {
            frame.bind(name, Binding::Var(value.to_json().unwrap()));
        }
    }

    Ok(Value::Undefined)
}

fn evaluate_numeric_expression(
    node: &Node,
    input: &Value,
    frame: &mut Frame,
    op: &BinaryOp,
) -> JsonAtaResult<Value> {
    let lhs = evaluate(&node.children[0], input, frame)?;
    let rhs = evaluate(&node.children[1], input, frame)?;

    let lhs: f64 = match lhs.as_raw() {
        JsonValue::Number(value) => value.clone().into(),
        _ => {
            return Err(Box::new(T2001 {
                position: node.position,
                op: op.to_string(),
            }))
        }
    };

    let rhs: f64 = match rhs.as_raw() {
        JsonValue::Number(value) => value.clone().into(),
        _ => {
            return Err(Box::new(T2002 {
                position: node.position,
                op: op.to_string(),
            }))
        }
    };

    let result = match op {
        BinaryOp::Add => lhs + rhs,
        BinaryOp::Subtract => lhs - rhs,
        BinaryOp::Multiply => lhs * rhs,
        BinaryOp::Divide => lhs / rhs,
        BinaryOp::Modulus => lhs % rhs,
        _ => unreachable!(),
    };

    Ok(Value::Raw(result.into()))
}

fn evaluate_comparison_expression(
    node: &Node,
    input: &Value,
    frame: &mut Frame,
    op: &BinaryOp,
) -> JsonAtaResult<Value> {
    let lhs = evaluate(&node.children[0], input, frame)?;
    let rhs = evaluate(&node.children[1], input, frame)?;

    let lhs = match lhs {
        Value::Undefined => return Ok(Value::Undefined),
        _ => lhs.as_raw(),
    };

    let rhs = match rhs {
        Value::Undefined => return Ok(Value::Undefined),
        _ => rhs.as_raw(),
    };

    if !((lhs.is_number() || lhs.is_string()) && (rhs.is_number() || rhs.is_string())) {
        return Err(Box::new(T2010 {
            position: node.position,
            op: op.to_string(),
        }));
    }

    if lhs.is_number() && rhs.is_number() {
        let lhs = lhs.as_f64().unwrap();
        let rhs = rhs.as_f64().unwrap();

        return Ok(Value::Raw(json::from(match op {
            BinaryOp::LessThan => lhs < rhs,
            BinaryOp::LessThanEqual => lhs <= rhs,
            BinaryOp::GreaterThan => lhs > rhs,
            BinaryOp::GreaterThanEqual => lhs >= rhs,
            _ => unreachable!(),
        })));
    }

    if lhs.is_string() && rhs.is_string() {
        let lhs = lhs.as_str().unwrap();
        let rhs = rhs.as_str().unwrap();

        return Ok(Value::Raw(json::from(match op {
            BinaryOp::LessThan => lhs < rhs,
            BinaryOp::LessThanEqual => lhs <= rhs,
            BinaryOp::GreaterThan => lhs > rhs,
            BinaryOp::GreaterThanEqual => lhs >= rhs,
            _ => unreachable!(),
        })));
    }

    Err(Box::new(T2009 {
        position: node.position,
        lhs: lhs.to_string(),
        rhs: rhs.to_string(),
        op: op.to_string(),
    }))
}

fn evaluate_boolean_expression(
    node: &Node,
    input: &Value,
    frame: &mut Frame,
    op: &BinaryOp,
) -> JsonAtaResult<Value> {
    let lhs = evaluate(&node.children[0], input, frame)?;
    let rhs = evaluate(&node.children[1], input, frame)?;

    let left_bool = boolean(&lhs);
    let right_bool = boolean(&rhs);

    let result = match op {
        BinaryOp::And => left_bool && right_bool,
        BinaryOp::Or => left_bool || right_bool,
        _ => unreachable!(),
    };

    Ok(Value::Raw(result.into()))
}

fn evaluate_includes_expression(
    node: &Node,
    input: &Value,
    frame: &mut Frame,
) -> JsonAtaResult<Value> {
    let lhs = evaluate(&node.children[0], input, frame)?;
    let rhs = evaluate(&node.children[1], input, frame)?;

    if !rhs.is_array() {
        return Ok(Value::Raw((lhs.as_raw() == rhs.as_raw()).into()));
    }

    for item in rhs.iter() {
        if item.is_raw() && lhs.as_raw() == item.as_raw() {
            return Ok(Value::Raw(true.into()));
        }
    }

    return Ok(Value::Raw(false.into()));
}

fn evaluate_equality_expression(
    node: &Node,
    input: &Value,
    frame: &mut Frame,
    op: &BinaryOp,
) -> JsonAtaResult<Value> {
    let lhs = evaluate(&node.children[0], input, frame)?;
    let rhs = evaluate(&node.children[1], input, frame)?;

    let result = match op {
        BinaryOp::Equal => lhs == rhs,
        BinaryOp::NotEqual => lhs != rhs,
        _ => unreachable!(),
    };

    Ok(Value::Raw(result.into()))
}

fn evaluate_string_concat(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    let lhs = evaluate(&node.children[0], input, frame)?;
    let rhs = evaluate(&node.children[1], input, frame)?;

    let mut lstr = string(lhs).unwrap();
    let rstr = string(rhs).unwrap();

    lstr.push_str(&rstr);

    Ok(Value::Raw(lstr.into()))
}

fn evaluate_path(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    let mut input = if input.is_array() {
        input.clone()
    } else {
        Value::new_seq_from(input)
    };

    // TODO: Tuple, singleton array, group expressions (jsonata.js:164)

    let mut result = Value::Undefined;

    for (step_index, step) in node.children.iter().enumerate() {
        result = evaluate_step(step, &input, frame, step_index == node.children.len() - 1)?;

        match result {
            Value::Undefined => break,
            Value::Raw(..) => panic!("unexpected Value::Raw"),
            Value::Array { .. } => {
                if result.is_empty() {
                    break;
                }

                input = result.clone();
            }
        }
    }

    Ok(result)
}

fn evaluate_step(
    node: &Node,
    input: &Value,
    frame: &mut Frame,
    last_step: bool,
) -> JsonAtaResult<Value> {
    // TODO: Sorting (jsonata.js:253)

    let mut result = Value::new_seq();

    for input in input.iter() {
        let res = evaluate(node, input, frame)?;

        // TODO: Filtering (jsonata.js:267)

        if !res.is_undef() {
            result.push(res);
        }
    }

    //println!("evaluate_step RESULT: {:#?}", result);

    if last_step && result.len() == 1 && result[0].is_array() && !result[0].is_seq() {
        Ok(result[0].clone())
    } else {
        // Flatten the result
        let mut flattened = Value::new_seq();
        result.iter().cloned().for_each(|v| {
            if !v.is_array() || v.keep_array() {
                flattened.push(v.clone())
            } else {
                v.iter().cloned().for_each(|v| flattened.push(v.clone()))
            }
        });
        Ok(flattened)
    }
}

fn evaluate_block(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    if let NodeKind::Block = &node.kind {
        let mut frame = Frame::new_with_parent(frame);
        let mut result = Value::Undefined;

        for child in &node.children {
            result = evaluate(child, input, &mut frame)?;
        }

        Ok(result)
    } else {
        panic!("`node` should be a NodeKind::Block");
    }
}

fn evaluate_ternary(node: &Node, input: &Value, frame: &mut Frame) -> JsonAtaResult<Value> {
    if let NodeKind::Ternary = &node.kind {
        let condition = evaluate(&node.children[0], input, frame)?;
        if boolean(&condition) {
            evaluate(&node.children[1], input, frame)
        } else if node.children.len() > 2 {
            evaluate(&node.children[2], input, frame)
        } else {
            Ok(Value::Undefined)
        }
    } else {
        panic!("`node` should be a NodeKind::Ternary")
    }
}

fn evaluate_variable(name: &str, frame: &Frame) -> JsonAtaResult<Value> {
    // TODO: Handle empty var name for $ context (jsonata.js:1143)
    if let Some(binding) = frame.lookup(name) {
        Ok(Value::Raw(binding.as_var().clone()))
    } else {
        Ok(Value::Undefined)
    }
}
