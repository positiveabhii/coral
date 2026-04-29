//! Build script for generated source-spec manifest protobuf types.

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);

    configure_serde(&mut config);

    config
        .compile_protos(&["proto/coral/spec/v1/source.proto"], &["proto"])
        .expect("compile coral spec protobuf");
}

#[allow(
    clippy::too_many_lines,
    reason = "Serde configuration is a declarative list of generated proto fields."
)]
fn configure_serde(config: &mut prost_build::Config) {
    let checked_message = "#[derive(serde::Deserialize)]\n#[serde(default, deny_unknown_fields)]";
    let flexible_message = "#[derive(serde::Deserialize)]\n#[serde(default)]";

    for message in [
        "SourceManifest",
        "SourceInputBinding",
        "SourceInput",
        "BasicAuthSpec",
        "HeaderAuthSpec",
        "RateLimitSpec",
        "TableSpec",
        "FilterSpec",
        "RequestSpec",
        "TemplateValue",
        "LiteralValue",
        "FilterValue",
        "FilterIntValue",
        "FilterBoolValue",
        "InputValue",
        "StateValue",
        "NowEpochMinusSecondsValue",
        "ResponseSpec",
        "PaginationSpec",
        "PageSizeSpec",
        "FileSourceSpec",
        "PartitionColumnSpec",
        "ColumnSpec",
        "PathExpr",
        "CoalesceExpr",
        "FromFilterExpr",
        "LiteralExpr",
        "NullExpr",
        "JoinArrayExpr",
        "TagValueExpr",
        "IfPresentExpr",
        "JoinTagValuesExpr",
        "FirstArrayItemPathExpr",
        "ObjectFilterPathExpr",
        "CurrentRowExpr",
        "FormatTimestampExpr",
        "ReplaceExpr",
        "TemplateExpr",
    ] {
        config.type_attribute(format!("coral.spec.v1.{message}"), checked_message);
    }

    for message in [
        "HeaderSpec",
        "QueryParamSpec",
        "BodyFieldSpec",
        "ValueSource",
    ] {
        config.type_attribute(format!("coral.spec.v1.{message}"), flexible_message);
    }

    config.type_attribute(
        "coral.spec.v1.AuthSpec.kind",
        "#[derive(serde::Deserialize)]\n#[serde(tag = \"type\")]",
    );
    config.field_attribute(
        "coral.spec.v1.AuthSpec.basic",
        "#[serde(rename = \"BasicAuth\")]",
    );
    config.field_attribute(
        "coral.spec.v1.AuthSpec.header",
        "#[serde(rename = \"HeaderAuth\")]",
    );
    config.field_attribute(
        "coral.spec.v1.AuthSpec.custom",
        "#[serde(rename = \"CustomAuth\")]",
    );

    config.type_attribute(
        "coral.spec.v1.ValueSource.kind",
        "#[derive(serde::Deserialize)]\n#[serde(tag = \"from\", rename_all = \"snake_case\")]",
    );
    config.field_attribute("coral.spec.v1.ValueSource.kind", "#[serde(flatten)]");

    config.type_attribute(
        "coral.spec.v1.ExprSpec.kind",
        "#[derive(serde::Deserialize)]\n#[serde(tag = \"kind\", rename_all = \"snake_case\")]",
    );

    config.field_attribute(
        "coral.spec.v1.SourceManifest.backend",
        "#[serde(default, deserialize_with = \"crate::proto_serde::source_backend\")]",
    );
    config.field_attribute(
        "coral.spec.v1.SourceManifest.inputs",
        "#[serde(default, deserialize_with = \"crate::proto_serde::source_inputs\")]",
    );

    config.field_attribute(
        "coral.spec.v1.SourceInput.kind",
        "#[serde(default, deserialize_with = \"crate::proto_serde::source_input_kind\")]",
    );
    config.field_attribute(
        "coral.spec.v1.SourceInput.default_value",
        "#[serde(default, rename = \"default\")]",
    );

    config.field_attribute(
        "coral.spec.v1.HeaderSpec.value",
        "#[serde(default, flatten)]",
    );
    config.field_attribute(
        "coral.spec.v1.QueryParamSpec.value",
        "#[serde(default, flatten)]",
    );
    config.field_attribute(
        "coral.spec.v1.BodyFieldSpec.value",
        "#[serde(default, flatten)]",
    );

    config.field_attribute(
        "coral.spec.v1.FilterSpec.mode",
        "#[serde(default, deserialize_with = \"crate::proto_serde::filter_mode\")]",
    );
    config.field_attribute(
        "coral.spec.v1.RequestSpec.method",
        "#[serde(default, deserialize_with = \"crate::proto_serde::http_method\")]",
    );
    config.field_attribute(
        "coral.spec.v1.RequestSpec.body",
        "#[serde(default, deserialize_with = \"crate::proto_serde::request_body\")]",
    );

    config.field_attribute(
        "coral.spec.v1.LiteralValue.json",
        "#[serde(rename = \"value\", deserialize_with = \"crate::proto_serde::json_string\")]",
    );
    config.field_attribute(
        "coral.spec.v1.FilterValue.default_json",
        "#[serde(default, rename = \"default\", deserialize_with = \"crate::proto_serde::optional_json_string\")]",
    );
    config.field_attribute(
        "coral.spec.v1.FilterIntValue.default_value",
        "#[serde(default, rename = \"default\")]",
    );
    config.field_attribute(
        "coral.spec.v1.FilterBoolValue.default_value",
        "#[serde(default, rename = \"default\")]",
    );

    config.field_attribute(
        "coral.spec.v1.ResponseSpec.format",
        "#[serde(default, deserialize_with = \"crate::proto_serde::response_body_format\")]",
    );
    config.field_attribute(
        "coral.spec.v1.ResponseSpec.row_strategy",
        "#[serde(default, deserialize_with = \"crate::proto_serde::row_strategy\")]",
    );
    config.field_attribute(
        "coral.spec.v1.PaginationSpec.mode",
        "#[serde(default, deserialize_with = \"crate::proto_serde::pagination_mode\")]",
    );
    config.field_attribute(
        "coral.spec.v1.PageSizeSpec.default_size",
        "#[serde(rename = \"default\")]",
    );

    config.field_attribute(
        "coral.spec.v1.PartitionColumnSpec.data_type",
        "#[serde(rename = \"type\")]",
    );
    config.field_attribute(
        "coral.spec.v1.ColumnSpec.data_type",
        "#[serde(rename = \"type\")]",
    );
    config.field_attribute(
        "coral.spec.v1.LiteralExpr.json",
        "#[serde(rename = \"value\", deserialize_with = \"crate::proto_serde::json_string\")]",
    );
    config.field_attribute(
        "coral.spec.v1.FormatTimestampExpr.input",
        "#[serde(default, deserialize_with = \"crate::proto_serde::timestamp_input\")]",
    );
}
