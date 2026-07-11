//! YAML support built on `serde-saphyr`.

use confique::{
	Config,
	meta::{Expr, FieldKind, LeafKind, Meta},
};
use core::fmt::{self, Display, Write as _};
use serde::Deserialize;
use serde_saphyr::{FlowMap, FlowSeq};

/// Deserializes a configuration layer with consistent YAML 1.2 behavior.
pub(super) fn from_str<'de, T>(input: &'de str) -> Result<T, serde_saphyr::Error>
where
	T: Deserialize<'de>,
{
	let options = serde_saphyr::options! {
		strict_booleans: true,
	};
	serde_saphyr::from_str_with_options(input, options)
}

/// Renders the default YAML configuration template.
pub(super) fn template<C: Config>() -> String {
	let mut renderer = TemplateRenderer::default();
	for doc in C::META.doc {
		renderer.comment(doc);
	}
	renderer.gap();
	renderer.meta(&C::META);
	renderer.finish()
}

/// YAML renderer for `confique`'s public configuration metadata.
#[derive(Default)]
struct TemplateRenderer {
	/// Rendered YAML accumulated so far.
	buffer: String,
	/// Indentation for the current nested configuration section.
	indentation: String,
}

impl TemplateRenderer {
	/// Renders all leaf fields followed by nested sections.
	fn meta(&mut self, meta: &Meta) {
		let mut emitted = false;
		for field in meta.fields {
			let FieldKind::Leaf { env, kind } = &field.kind else {
				continue;
			};
			if emitted {
				self.gap();
			}
			emitted = true;

			let mut emitted_comment = false;
			for doc in field.doc {
				self.comment(doc);
				emitted_comment = true;
			}
			if let Some(env) = env {
				self.separate_comments(emitted_comment);
				self.comment(format_args!(
					" Can also be specified via environment variable `{env}`."
				));
				emitted_comment = true;
			}

			match kind {
				LeafKind::Optional => self.disabled_field(field.name, None),
				LeafKind::Required { default } => {
					self.separate_comments(emitted_comment);
					match default {
						Some(value) => {
							self.comment(format_args!(" Default value: {}", YamlExpr(value)));
						}
						None => self.comment(" Required! This value must be specified."),
					}
					self.disabled_field(field.name, default.as_ref());
				}
			}
		}

		for field in meta.fields {
			let FieldKind::Nested { meta: nested_meta } = field.kind else {
				continue;
			};
			if emitted {
				self.gap();
			}
			emitted = true;
			for doc in field.doc {
				self.comment(doc);
			}
			self.indent();
			writeln!(self.buffer, "{}:", field.name).expect("writing to a string cannot fail");
			self.indentation.push_str("  ");
			self.meta(nested_meta);
			self.indentation = self
				.indentation
				.strip_suffix("  ")
				.expect("nested section must add indentation")
				.to_owned();
		}
	}

	/// Adds a blank comment between two comment groups.
	fn separate_comments(&mut self, emitted_comment: bool) {
		if emitted_comment {
			self.comment("");
		}
	}

	/// Writes a YAML comment at the current indentation.
	fn comment(&mut self, comment: impl Display) {
		self.indent();
		writeln!(self.buffer, "#{comment}").expect("writing to a string cannot fail");
	}

	/// Writes a commented-out field and its optional default.
	fn disabled_field(&mut self, name: &str, value: Option<&Expr>) {
		self.indent();
		match value {
			Some(value) => writeln!(self.buffer, "#{name}: {}", YamlExpr(value)),
			None => writeln!(self.buffer, "#{name}:"),
		}
		.expect("writing to a string cannot fail");
	}

	/// Ensures one blank line separates template entries.
	fn gap(&mut self) {
		if !self.buffer.is_empty() && !self.buffer.ends_with("\n\n") {
			self.buffer.push('\n');
		}
	}

	/// Writes indentation for the current nested configuration depth.
	fn indent(&mut self) {
		self.buffer.push_str(&self.indentation);
	}

	/// Returns the completed template with one trailing newline.
	fn finish(mut self) -> String {
		while self.buffer.ends_with("\n\n") {
			self.buffer.pop();
		}
		if !self.buffer.ends_with('\n') {
			self.buffer.push('\n');
		}
		self.buffer
	}
}

/// Displays a metadata expression as compact YAML.
struct YamlExpr<'a>(&'a Expr);

impl Display for YamlExpr<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let yaml = match self.0 {
			Expr::Array(_) => serde_saphyr::to_string(&FlowSeq(self.0)),
			Expr::Map(_) => serde_saphyr::to_string(&FlowMap(self.0)),
			Expr::Str(_) | Expr::Float(_) | Expr::Integer(_) | Expr::Bool(_) | _ => {
				serde_saphyr::to_string(self.0)
			}
		}
		.map_err(|_error| fmt::Error)?;
		f.write_str(yaml.trim_end_matches('\n'))
	}
}

#[cfg(test)]
mod tests {
	use super::from_str;
	use pretty_assertions::assert_eq;
	use serde::Deserialize;

	#[derive(Debug, Deserialize, PartialEq, Eq)]
	struct Example {
		enabled: bool,
	}

	#[test]
	fn rejects_duplicate_keys() {
		let error = from_str::<Example>("enabled: true\nenabled: false\n")
			.expect_err("duplicate keys must be rejected");
		assert!(error.to_string().contains("duplicate"));
	}

	#[test]
	fn uses_strict_yaml_1_2_booleans() {
		let parsed = from_str::<Example>("enabled: true\n").expect("true is a YAML 1.2 boolean");
		assert_eq!(parsed, Example { enabled: true });
		from_str::<Example>("enabled: yes\n")
			.expect_err("YAML 1.1 boolean spellings must be rejected");
	}
}
