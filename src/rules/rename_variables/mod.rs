mod function_names;
mod globals;
mod rename_processor;

use rename_processor::RenameProcessor;

use crate::nodes::{Block, Expression, Identifier, Statement, Variable};
use crate::process::utils::is_valid_identifier;
use crate::process::{DefaultVisitor, NodeProcessor, NodeVisitor, ScopeVisitor};
use crate::rules::{
    Context, FlawlessRule, RuleConfiguration, RuleConfigurationError, RuleProperties,
    RulePropertyValue,
};

use std::collections::HashSet;
use std::iter::FromIterator;

pub const RENAME_VARIABLES_RULE_NAME: &str = "rename_variables";

/// Rename all identifiers to small and meaningless names.
#[derive(Debug, PartialEq, Eq)]
pub struct RenameVariables {
    additional_globals: Vec<String>,
    include_functions: bool,
}

impl RenameVariables {
    pub fn new<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self {
            additional_globals: Vec::from_iter(iter),
            include_functions: false,
        }
    }

    pub fn with_function_names(mut self) -> Self {
        self.include_functions = true;
        self
    }

    fn set_additional_globals(&mut self, list: Vec<String>) -> Result<(), RuleConfigurationError> {
        for value in list {
            match value.as_str() {
                "$default" => self
                    .additional_globals
                    .extend(globals::DEFAULT.iter().map(ToString::to_string)),
                "$roblox" => self
                    .additional_globals
                    .extend(globals::ROBLOX.iter().map(ToString::to_string)),
                identifier if !is_valid_identifier(identifier) => {
                    return Err(RuleConfigurationError::StringExpected("".to_owned()))
                }
                _ => self.additional_globals.push(value),
            }
        }

        Ok(())
    }

    fn normalize_additional_globals(&self) -> Vec<String> {
        let mut globals_set: HashSet<String> = self.additional_globals.iter().cloned().collect();

        let mut result = Vec::new();

        if globals::DEFAULT
            .iter()
            .all(|identifier| globals_set.contains(*identifier))
        {
            globals::DEFAULT.iter().for_each(|identifier| {
                globals_set.remove(*identifier);
            });
            result.push("$default".to_owned());
        }

        if globals::ROBLOX
            .iter()
            .all(|identifier| globals_set.contains(*identifier))
        {
            globals::ROBLOX.iter().for_each(|identifier| {
                globals_set.remove(*identifier);
            });
            result.push("$roblox".to_owned());
        }

        result.extend(globals_set);
        result.sort();
        result
    }
}

impl Default for RenameVariables {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl FlawlessRule for RenameVariables {
    fn flawless_process(&self, block: &mut Block, _: &Context) {
        // Collect all global variables actually used in the file
        let mut collect_globals = CollectGlobalReferences::default();
        collect_globals.enter_scope();
        collect_globals.visit_block_ref(block);
        collect_globals.exit_scope();
        let used_globals = collect_globals.into_globals();

        let avoid_identifiers = if self.include_functions {
            Vec::new()
        } else {
            let mut collect_functions = function_names::CollectFunctionNames::default();
            DefaultVisitor::visit_block(block, &mut collect_functions);
            collect_functions.into()
        };

        let mut processor = RenameProcessor::new(
            used_globals
                .into_iter()
                .chain(self.additional_globals.clone())
                .chain(avoid_identifiers),
            self.include_functions,
        );
        ScopeVisitor::visit_block(block, &mut processor);
    }
}

impl RuleConfiguration for RenameVariables {
    fn configure(&mut self, properties: RuleProperties) -> Result<(), RuleConfigurationError> {
        for (key, value) in properties {
            match key.as_str() {
                "globals" => {
                    self.set_additional_globals(value.expect_string_list(&key)?)?;
                }
                "include_functions" => {
                    self.include_functions = value.expect_bool(&key)?;
                }
                _ => return Err(RuleConfigurationError::UnexpectedProperty(key)),
            }
        }

        Ok(())
    }

    fn get_name(&self) -> &'static str {
        RENAME_VARIABLES_RULE_NAME
    }

    fn serialize_to_properties(&self) -> RuleProperties {
        let mut properties = RuleProperties::new();

        let globals = self.normalize_additional_globals();
        if !globals.is_empty() {
            properties.insert("globals".to_owned(), RulePropertyValue::StringList(globals));
        }

        if self.include_functions {
            properties.insert(
                "include_functions".to_owned(),
                RulePropertyValue::Boolean(self.include_functions),
            );
        }

        properties
    }
}

/// Visitor to collect all global variable references in the code
#[derive(Debug, Default)]
struct CollectGlobalReferences {
    globals: HashSet<String>,
    local_scopes: Vec<HashSet<String>>,
}

impl CollectGlobalReferences {
    fn into_globals(self) -> Vec<String> {
        let mut globals: Vec<String> = self.globals.into_iter().collect();
        globals.sort();
        globals
    }

    fn is_local(&self, name: &str) -> bool {
        self.local_scopes.iter().any(|scope| scope.contains(name))
    }

    fn add_local(&mut self, name: String) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name);
        }
    }

    fn enter_scope(&mut self) {
        self.local_scopes.push(HashSet::new());
    }

    fn exit_scope(&mut self) {
        self.local_scopes.pop();
    }

    fn visit_identifier_ref(&mut self, identifier: &Identifier) {
        let name = identifier.get_name();
        if !self.is_local(name) {
            self.globals.insert(name.to_string());
        }
    }

    fn visit_variable_ref(&mut self, variable: &Variable) {
        match variable {
            Variable::Identifier(identifier) => {
                self.visit_identifier_ref(identifier);
            }
            Variable::Field(field) => {
                self.visit_prefix_ref(field.get_prefix());
            }
            Variable::Index(index) => {
                self.visit_prefix_ref(index.get_prefix());
                self.visit_expression_ref(index.get_index());
            }
        }
    }

    fn visit_prefix_ref(&mut self, prefix: &crate::nodes::Prefix) {
        match prefix {
            crate::nodes::Prefix::Identifier(identifier) => {
                self.visit_identifier_ref(identifier);
            }
            crate::nodes::Prefix::Field(field) => {
                self.visit_prefix_ref(field.get_prefix());
            }
            crate::nodes::Prefix::Index(index) => {
                self.visit_prefix_ref(index.get_prefix());
                self.visit_expression_ref(index.get_index());
            }
            crate::nodes::Prefix::Parenthese(paren) => {
                self.visit_expression_ref(paren.inner_expression());
            }
            crate::nodes::Prefix::Call(call) => {
                self.visit_prefix_ref(call.get_prefix());
                self.visit_arguments_ref(call.get_arguments());
            }
        }
    }

    fn visit_arguments_ref(&mut self, arguments: &crate::nodes::Arguments) {
        match arguments {
            crate::nodes::Arguments::Tuple(tuple) => {
                for arg in tuple.iter_values() {
                    self.visit_expression_ref(arg);
                }
            }
            crate::nodes::Arguments::String(_) => {}
            crate::nodes::Arguments::Table(table) => {
                for entry in table.iter_entries() {
                    match entry {
                        crate::nodes::TableEntry::Field(field) => {
                            self.visit_expression_ref(field.get_value());
                        }
                        crate::nodes::TableEntry::Index(index) => {
                            self.visit_expression_ref(index.get_key());
                            self.visit_expression_ref(index.get_value());
                        }
                        crate::nodes::TableEntry::Value(value) => {
                            self.visit_expression_ref(value);
                        }
                    }
                }
            }
        }
    }

    fn visit_expression_ref(&mut self, expression: &Expression) {
        match expression {
            Expression::Identifier(identifier) => {
                self.visit_identifier_ref(identifier);
            }
            Expression::Parenthese(inner) => {
                self.visit_expression_ref(inner.inner_expression());
            }
            Expression::Unary(unary) => {
                self.visit_expression_ref(unary.get_expression());
            }
            Expression::Binary(binary) => {
                self.visit_expression_ref(binary.left());
                self.visit_expression_ref(binary.right());
            }
            Expression::Function(function) => {
                self.enter_scope();
                for param in function.iter_parameters() {
                    self.add_local(param.get_name().to_string());
                }
                self.visit_block_ref(function.get_block());
                self.exit_scope();
            }
            Expression::Call(call) => {
                self.visit_prefix_ref(call.get_prefix());
                self.visit_arguments_ref(call.get_arguments());
            }
            Expression::Table(table) => {
                for entry in table.iter_entries() {
                    match entry {
                        crate::nodes::TableEntry::Field(field) => {
                            self.visit_expression_ref(field.get_value());
                        }
                        crate::nodes::TableEntry::Index(index) => {
                            self.visit_expression_ref(index.get_key());
                            self.visit_expression_ref(index.get_value());
                        }
                        crate::nodes::TableEntry::Value(value) => {
                            self.visit_expression_ref(value);
                        }
                    }
                }
            }
            Expression::If(if_expr) => {
                self.visit_expression_ref(if_expr.get_condition());
                self.visit_expression_ref(if_expr.get_result());
                for branch in if_expr.iter_branches() {
                    self.visit_expression_ref(branch.get_condition());
                    self.visit_expression_ref(branch.get_result());
                }
                self.visit_expression_ref(if_expr.get_else_result());
            }
            Expression::InterpolatedString(_interpolated) => {
                // TODO: Visit interpolated expressions when API is determined
                // InterpolationSegment doesn't expose a public method to access the expression
            }
            Expression::TypeCast(cast) => {
                self.visit_expression_ref(cast.get_expression());
            }
            _ => {}
        }
    }

    fn visit_block_ref(&mut self, block: &Block) {
        // First pass: collect all local declarations in this block
        let mut locals = Vec::new();
        for statement in block.iter_statements() {
            match statement {
                Statement::LocalAssign(local_assign) => {
                    for var in local_assign.iter_variables() {
                        locals.push(var.get_name().to_string());
                    }
                }
                Statement::LocalFunction(local_func) => {
                    locals.push(local_func.get_identifier().get_name().to_string());
                }
                _ => {}
            }
        }

        // Add locals to current scope
        for local in locals {
            self.add_local(local);
        }

        // Second pass: visit statements
        for statement in block.iter_statements() {
            self.visit_statement_ref(statement);
        }

        // Visit last statement
        if let Some(last_stmt) = block.get_last_statement() {
            match last_stmt {
                crate::nodes::LastStatement::Return(ret) => {
                    for expr in ret.iter_expressions() {
                        self.visit_expression_ref(expr);
                    }
                }
                _ => {}
            }
        }
    }

    fn visit_statement_ref(&mut self, statement: &Statement) {
        match statement {
            Statement::Assign(assign) => {
                for var in assign.iter_variables() {
                    self.visit_variable_ref(var);
                }
                for expr in assign.iter_values() {
                    self.visit_expression_ref(expr);
                }
            }
            Statement::Do(do_stmt) => {
                self.enter_scope();
                self.visit_block_ref(do_stmt.get_block());
                self.exit_scope();
            }
            Statement::CompoundAssign(compound) => {
                self.visit_variable_ref(compound.get_variable());
                self.visit_expression_ref(compound.get_value());
            }
            Statement::Call(call) => {
                self.visit_prefix_ref(call.get_prefix());
                self.visit_arguments_ref(call.get_arguments());
            }
            Statement::GenericFor(generic_for) => {
                // Visit expressions first (can reference globals)
                for expr in generic_for.iter_expressions() {
                    self.visit_expression_ref(expr);
                }

                // Loop variables are local to the loop
                self.enter_scope();
                for id in generic_for.iter_identifiers() {
                    self.add_local(id.get_name().to_string());
                }
                self.visit_block_ref(generic_for.get_block());
                self.exit_scope();
            }
            Statement::If(if_stmt) => {
                for branch in if_stmt.iter_branches() {
                    self.visit_expression_ref(branch.get_condition());
                    self.enter_scope();
                    self.visit_block_ref(branch.get_block());
                    self.exit_scope();
                }

                if let Some(else_block) = if_stmt.get_else_block() {
                    self.enter_scope();
                    self.visit_block_ref(else_block);
                    self.exit_scope();
                }
            }
            Statement::LocalAssign(local_assign) => {
                // Visit values (can reference globals)
                for expr in local_assign.iter_values() {
                    self.visit_expression_ref(expr);
                }
                // Variables already added in visit_block_ref
            }
            Statement::LocalFunction(local_func) => {
                // Identifier already added in visit_block_ref
                self.enter_scope();
                for param in local_func.iter_parameters() {
                    self.add_local(param.get_name().to_string());
                }
                self.visit_block_ref(local_func.get_block());
                self.exit_scope();
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expression_ref(numeric_for.get_start());
                self.visit_expression_ref(numeric_for.get_end());
                if let Some(step) = numeric_for.get_step() {
                    self.visit_expression_ref(step);
                }

                self.enter_scope();
                self.add_local(numeric_for.get_identifier().get_name().to_string());
                self.visit_block_ref(numeric_for.get_block());
                self.exit_scope();
            }
            Statement::Repeat(repeat) => {
                self.enter_scope();
                self.visit_block_ref(repeat.get_block());
                self.visit_expression_ref(repeat.get_condition());
                self.exit_scope();
            }
            Statement::While(while_stmt) => {
                self.visit_expression_ref(while_stmt.get_condition());
                self.enter_scope();
                self.visit_block_ref(while_stmt.get_block());
                self.exit_scope();
            }
            Statement::Function(func_stmt) => {
                self.visit_function_name_ref(func_stmt.get_name());
                self.enter_scope();
                for param in func_stmt.iter_parameters() {
                    self.add_local(param.get_name().to_string());
                }
                self.visit_block_ref(func_stmt.get_block());
                self.exit_scope();
            }
            Statement::TypeDeclaration(_) => {}
            Statement::TypeFunction(_) => {}
        }
    }

    fn visit_function_name_ref(&mut self, name: &crate::nodes::FunctionName) {
        self.visit_identifier_ref(name.get_name());
        // Field names and method names are not variables, they're table keys
    }
}

impl NodeProcessor for CollectGlobalReferences {
    fn process_block(&mut self, _block: &mut Block) {}
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::rules::Rule;

    use insta::assert_json_snapshot;
    use std::iter::empty;

    fn new_rule() -> Box<dyn Rule> {
        Box::<RenameVariables>::default()
    }

    #[test]
    fn serialize_default_rule() {
        assert_json_snapshot!("default_rename_variables", new_rule());
    }

    #[test]
    fn serialize_no_globals_rule() {
        assert_json_snapshot!(
            "no_globals_rename_variables",
            Box::new(RenameVariables::new(empty())) as Box<dyn Rule>
        );
    }

    #[test]
    fn serialize_roblox_globals_rule() {
        let rule = Box::new(RenameVariables::new(
            globals::ROBLOX.iter().map(ToString::to_string),
        ));

        assert_json_snapshot!("roblox_globals_rename_variables", rule as Box<dyn Rule>);
    }

    #[test]
    fn serialize_with_function_names() {
        let rule = Box::new(
            RenameVariables::new(globals::DEFAULT.iter().map(ToString::to_string))
                .with_function_names(),
        );

        assert_json_snapshot!(
            "rename_variables_with_function_names",
            rule as Box<dyn Rule>
        );
    }

    #[test]
    fn serialize_skip_functions() {
        let rule = Box::new(RenameVariables::new(
            globals::ROBLOX.iter().map(ToString::to_string),
        ));

        assert_json_snapshot!("roblox_globals_rename_variables", rule as Box<dyn Rule>);
    }
}
