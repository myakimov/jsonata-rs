use super::evaluator::Evaluator;
use super::frame::Frame;
use super::position::Position;
use super::value::{ArrayFlags, Value, ValueKind, ValuePool};
use super::{Error, Result};

#[derive(Clone)]
pub struct FunctionContext<'a> {
    pub name: &'a str,
    pub position: Position,
    pub pool: ValuePool,
    pub input: Value,
    pub frame: Frame,
    pub evaluator: &'a Evaluator,
}

impl<'a> FunctionContext<'a> {
    pub fn evaluate_function(&self, proc: &Value, args: &Value) -> Result<Value> {
        self.evaluator
            .apply_function(self.position, &self.input, proc, args, &self.frame)
    }
}

pub fn fn_lookup_internal(context: &FunctionContext, input: &Value, key: &str) -> Value {
    match **input {
        ValueKind::Array { .. } => {
            let mut result = context.pool.array(ArrayFlags::SEQUENCE);

            for input in input.members() {
                let res = fn_lookup_internal(context, &input, key);
                match *res {
                    ValueKind::Undefined => {}
                    ValueKind::Array { .. } => {
                        res.members().for_each(|item| result.push_index(item.index));
                    }
                    _ => result.push_index(res.index),
                };
            }

            result
        }
        ValueKind::Object(..) => input.get_entry(key),
        _ => context.pool.undefined(),
    }
}

pub fn fn_lookup(context: &FunctionContext, input: &Value, key: &Value) -> Result<Value> {
    if !key.is_string() {
        Err(Error::argument_not_valid(context, 1))
    } else {
        Ok(fn_lookup_internal(context, input, &key.as_str()))
    }
}

pub fn fn_append(context: &FunctionContext, arg1: &Value, arg2: &Value) -> Result<Value> {
    if arg1.is_undefined() {
        return Ok(arg2.clone());
    }

    if arg2.is_undefined() {
        return Ok(arg1.clone());
    }

    let result = context.pool.value((**arg1).clone());
    let mut result = result.wrap_in_array_if_needed(ArrayFlags::SEQUENCE);
    let arg2 = arg2.wrap_in_array_if_needed(ArrayFlags::empty());
    arg2.members().for_each(|m| result.push_index(m.index));

    Ok(result)
}

pub fn fn_boolean(context: &FunctionContext, arg: &Value) -> Result<Value> {
    Ok(match **arg {
        ValueKind::Undefined => context.pool.undefined(),
        ValueKind::Null => context.pool.bool(false),
        ValueKind::Bool(b) => context.pool.bool(b),
        ValueKind::Number(num) => context.pool.bool(num != 0.0),
        ValueKind::String(ref str) => context.pool.bool(!str.is_empty()),
        ValueKind::Object(ref obj) => context.pool.bool(!obj.is_empty()),
        ValueKind::Array { .. } => match arg.len() {
            0 => context.pool.bool(false),
            1 => fn_boolean(context, &arg.get_member(0))?,
            _ => {
                for item in arg.members() {
                    if fn_boolean(context, &item)?.as_bool() {
                        return Ok(context.pool.bool(true));
                    }
                }
                context.pool.bool(false)
            }
        },
        ValueKind::Lambda(..)
        | ValueKind::NativeFn0 { .. }
        | ValueKind::NativeFn1 { .. }
        | ValueKind::NativeFn2 { .. }
        | ValueKind::NativeFn3 { .. } => context.pool.bool(false),
    })
}

pub fn fn_filter(context: &FunctionContext, arr: &Value, func: &Value) -> Result<Value> {
    if arr.is_undefined() {
        return Ok(context.pool.undefined());
    }

    let arr = arr.wrap_in_array_if_needed(ArrayFlags::empty());

    if !func.is_function() {
        return Err(Error::argument_not_valid(context, 2));
    }

    let mut result = context.pool.array(ArrayFlags::SEQUENCE);

    for (index, item) in arr.members().enumerate() {
        let mut args = context.pool.array(ArrayFlags::empty());
        let arity = func.arity();

        args.push_index(item.index);
        if arity >= 2 {
            args.push(ValueKind::Number(index.into()));
        }
        if arity >= 3 {
            args.push_index(arr.index);
        }

        let include = context.evaluate_function(func, &args)?;

        if include.is_truthy() {
            result.push_index(item.index);
        }
    }

    Ok(result)
}

pub fn fn_string(context: &FunctionContext, arg: &Value) -> Result<Value> {
    if arg.is_undefined() {
        return Ok(context.pool.undefined());
    }

    if arg.is_string() {
        Ok(arg.clone())
    } else if arg.is_function() {
        Ok(context.pool.string(String::from("")))

    // TODO: Check for infinite numbers
    // } else if arg.is_number() && arg.is_infinite() {
    //     // TODO: D3001
    //     unreachable!()

    // TODO: pretty printing
    } else {
        Ok(context.pool.string(arg.dump()))
    }
}

pub fn fn_count(context: &FunctionContext, arg: &Value) -> Result<Value> {
    Ok(context.pool.number(if arg.is_undefined() {
        0
    } else if arg.is_array() {
        arg.len()
    } else {
        1
    }))
}

pub fn fn_not(context: &FunctionContext, arg: &Value) -> Result<Value> {
    Ok(if arg.is_undefined() {
        context.pool.undefined()
    } else {
        context.pool.bool(!arg.is_truthy())
    })
}

pub fn fn_lowercase(context: &FunctionContext, arg: &Value) -> Result<Value> {
    Ok(if !arg.is_string() {
        context.pool.undefined()
    } else {
        context.pool.string(arg.as_str().to_lowercase())
    })
}

pub fn fn_uppercase(context: &FunctionContext, arg: &Value) -> Result<Value> {
    if !arg.is_string() {
        Ok(context.pool.undefined())
    } else {
        Ok(context.pool.string(arg.as_str().to_uppercase()))
    }
}

pub fn fn_substring(
    context: &FunctionContext,
    string: &Value,
    start: &Value,
    length: &Value,
) -> Result<Value> {
    if string.is_undefined() {
        return Ok(context.pool.undefined());
    }

    if !string.is_string() {
        return Err(Error::argument_not_valid(context, 1));
    }

    if !start.is_number() {
        return Err(Error::argument_not_valid(context, 2));
    }

    let string = string.as_str();

    // Scan the string chars for the actual number of characters.
    // NOTE: Chars are not grapheme clusters, so for some inputs like "नमस्ते" we will get 6
    //       as it will include the diacritics.
    //       See: https://doc.rust-lang.org/nightly/book/ch08-02-strings.html
    let len = string.chars().count() as isize;
    let mut start = start.as_isize();

    // If start is negative and runs off the front of the string
    if len + start < 0 {
        start = 0;
    }

    // If start is negative, count from the end of the string
    let start = if start < 0 { len + start } else { start };

    if length.is_undefined() {
        Ok(context.pool.string(string[start as usize..].to_string()))
    } else {
        if !length.is_number() {
            return Err(Error::argument_not_valid(context, 3));
        }

        let length = length.as_isize();
        if length < 0 {
            Ok(context.pool.string(String::from("")))
        } else {
            let end = if start >= 0 {
                (start + length) as usize
            } else {
                (len + start + length) as usize
            };

            let substring = string
                .chars()
                .skip(start as usize)
                .take(end - start as usize)
                .collect::<String>();

            Ok(context.pool.string(substring))
        }
    }
}

pub fn fn_abs(context: &FunctionContext, arg: &Value) -> Result<Value> {
    if arg.is_undefined() {
        Ok(context.pool.undefined())
    } else if !arg.is_number() {
        Err(Error::argument_not_valid(context, 1))
    } else {
        Ok(context.pool.number(arg.as_f64().abs()))
    }
}

pub fn fn_floor(context: &FunctionContext, arg: &Value) -> Result<Value> {
    if arg.is_undefined() {
        Ok(context.pool.undefined())
    } else if !arg.is_number() {
        Err(Error::argument_not_valid(context, 1))
    } else {
        Ok(context.pool.number(arg.as_f64().floor()))
    }
}

pub fn fn_ceil(context: &FunctionContext, arg: &Value) -> Result<Value> {
    if arg.is_undefined() {
        Ok(context.pool.undefined())
    } else if !arg.is_number() {
        Err(Error::argument_not_valid(context, 1))
    } else {
        Ok(context.pool.number(arg.as_f64().ceil()))
    }
}

pub fn fn_max(context: &FunctionContext, args: &Value) -> Result<Value> {
    if args.is_undefined() || (args.is_array() && args.is_empty()) {
        return Ok(context.pool.undefined());
    }
    let args = args.wrap_in_array_if_needed(ArrayFlags::empty());
    let mut max = f64::MIN;
    for arg in args.members() {
        if !arg.is_number() {
            return Err(Error::argument_must_be_array_of_type(context, 2, "number"));
        }
        max = f64::max(max, arg.as_f64());
    }
    Ok(context.pool.number(max))
}

pub fn fn_min(context: &FunctionContext, args: &Value) -> Result<Value> {
    if args.is_undefined() || (args.is_array() && args.is_empty()) {
        return Ok(context.pool.undefined());
    }
    let args = args.wrap_in_array_if_needed(ArrayFlags::empty());
    let mut min = f64::MAX;
    for arg in args.members() {
        if !arg.is_number() {
            return Err(Error::argument_must_be_array_of_type(context, 2, "number"));
        }
        min = f64::min(min, arg.as_f64());
    }
    Ok(context.pool.number(min))
}

pub fn fn_sum(context: &FunctionContext, args: &Value) -> Result<Value> {
    if args.is_undefined() || (args.is_array() && args.is_empty()) {
        return Ok(context.pool.undefined());
    }
    let args = args.wrap_in_array_if_needed(ArrayFlags::empty());
    let mut sum = 0.0;
    for arg in args.members() {
        if !arg.is_number() {
            return Err(Error::argument_must_be_array_of_type(context, 2, "number"));
        }
        sum += arg.as_f64();
    }
    Ok(context.pool.number(sum))
}
