// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0b: the generated ONNX `Conv` graph of one template search (design §1.2
//! "模板匹配", §2.5 FS-G) and the small hand-written protobuf encoder that writes it; no ONNX or
//! protobuf crate is added. A reader of the same wire format describes a model's inputs,
//! initializers, nodes and outputs, so a logic test checks the graph's shape without ONNX
//! Runtime.
//!
//! The graph has one input, `image`: the frame's gray values (0..=255) as `f32`, shaped
//! `1x1xHxW`. For every scale `i` of the search it holds that scale's template as the initializer
//! `kernel_products_i` and computes `products_i = Conv(image, kernel_products_i)`, the dot product
//! `sum(T * I)` of every position (an ONNX `Conv` is a cross-correlation: no kernel flip; no
//! padding and stride 1 give every position where the template lies inside the frame). A masked
//! scale's products kernel is the masked template (`mask * T`); it also holds the mask (1 for a
//! selected pixel, 0 otherwise) as `kernel_mask_i` and computes `image_sums_i = Conv(image,
//! kernel_mask_i)` and `squared_sums_i = Conv(image_squared, kernel_mask_i)`, where
//! `image_squared = Mul(image, image)`. Every output is `1x1x(H - h + 1)x(W - w + 1)`.

use crate::error::{BenchCode, BenchError};

/// ONNX IR version 7 (ONNX 1.8), which opset 13 belongs to.
pub const IR_VERSION: u64 = 7;
/// The default-domain opset the graph imports.
pub const OPSET_VERSION: u64 = 13;
pub const PRODUCER_NAME: &str = "ac-vision-ep-bench";
/// The graph's only input.
pub const INPUT_NAME: &str = "image";
const SQUARED_NAME: &str = "image_squared";
/// `TensorProto.DataType.FLOAT`.
pub const ELEMENT_FLOAT: u64 = 1;
/// `AttributeProto.AttributeType.INTS`.
const ATTRIBUTE_INTS: u64 = 7;
const WIRE_VARINT: u8 = 0;
const WIRE_FIXED64: u8 = 1;
const WIRE_BYTES: u8 = 2;
const WIRE_FIXED32: u8 = 5;

/// The Conv graph of one template search over frames of one size.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvGraph {
    pub frame_width: u32,
    pub frame_height: u32,
    /// One kernel per scale of the search, in the declared order of the scales.
    pub scales: Vec<ConvKernel>,
}

/// The kernels of one scale, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvKernel {
    pub width: u32,
    pub height: u32,
    /// The template's gray values (the masked template's when `mask` is set).
    pub products: Vec<f32>,
    /// 1 for a pixel the mask selects, 0 otherwise.
    pub mask: Option<Vec<f32>>,
}

/// What an output of the graph holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvOutputKind {
    /// `sum(T * I)` (masked: `sum_m(T * I)`).
    Products,
    /// `sum_m(I)`.
    ImageSums,
    /// `sum_m(I^2)`.
    SquaredSums,
}

impl ConvOutputKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Products => "products",
            Self::ImageSums => "image_sums",
            Self::SquaredSums => "squared_sums",
        }
    }
}

/// The name of the output of `kind` for scale `scale_index`.
pub fn output_name(kind: ConvOutputKind, scale_index: usize) -> String {
    format!("{}_{scale_index}", kind.prefix())
}

fn products_kernel_name(scale_index: usize) -> String {
    format!("kernel_products_{scale_index}")
}

fn mask_kernel_name(scale_index: usize) -> String {
    format!("kernel_mask_{scale_index}")
}

impl ConvGraph {
    /// Every scale's kernels match its size, and every template lies inside the frame.
    pub fn validate(&self) -> Result<(), BenchError> {
        if self.frame_width == 0 || self.frame_height == 0 {
            return Err(graph_error("the frame size is zero"));
        }
        if self.scales.is_empty() {
            return Err(graph_error("the graph has no scale"));
        }
        for (index, kernel) in self.scales.iter().enumerate() {
            let pixels = u64::from(kernel.width) * u64::from(kernel.height);
            if kernel.width == 0
                || kernel.height == 0
                || kernel.width > self.frame_width
                || kernel.height > self.frame_height
            {
                return Err(graph_error(format!(
                    "scale {index}: the {}x{} template does not fit the {}x{} frame",
                    kernel.width, kernel.height, self.frame_width, self.frame_height
                )));
            }
            let matches = |values: &[f32]| values.len() as u64 == pixels;
            if !matches(kernel.products.as_slice())
                || kernel.mask.as_deref().is_some_and(|mask| !matches(mask))
            {
                return Err(graph_error(format!(
                    "scale {index}: a kernel does not hold {pixels} values"
                )));
            }
        }
        Ok(())
    }

    /// The input shape, `[1, 1, H, W]`.
    pub fn input_shape(&self) -> [i64; 4] {
        [
            1,
            1,
            i64::from(self.frame_height),
            i64::from(self.frame_width),
        ]
    }

    /// The output shape of scale `scale_index`, `[1, 1, H - h + 1, W - w + 1]`.
    pub fn output_shape(&self, scale_index: usize) -> Option<[i64; 4]> {
        let kernel = self.scales.get(scale_index)?;
        Some([
            1,
            1,
            i64::from(self.frame_height) - i64::from(kernel.height) + 1,
            i64::from(self.frame_width) - i64::from(kernel.width) + 1,
        ])
    }

    /// Every output, in graph order: per scale its products, then (masked) its image sums and
    /// squared sums.
    pub fn outputs(&self) -> Vec<(usize, ConvOutputKind, String)> {
        let mut outputs = Vec::new();
        for (index, kernel) in self.scales.iter().enumerate() {
            outputs.push((
                index,
                ConvOutputKind::Products,
                output_name(ConvOutputKind::Products, index),
            ));
            if kernel.mask.is_some() {
                for kind in [ConvOutputKind::ImageSums, ConvOutputKind::SquaredSums] {
                    outputs.push((index, kind, output_name(kind, index)));
                }
            }
        }
        outputs
    }

    /// The serialized `ModelProto`.
    pub fn encode(&self) -> Result<Vec<u8>, BenchError> {
        self.validate()?;
        let mut graph = Writer::default();
        let masked = self.scales.iter().any(|kernel| kernel.mask.is_some());
        if masked {
            graph.message(
                1,
                node(
                    "Mul",
                    "square",
                    &[INPUT_NAME, INPUT_NAME],
                    SQUARED_NAME,
                    None,
                ),
            );
        }
        for (index, kernel) in self.scales.iter().enumerate() {
            let shape = [i64::from(kernel.height), i64::from(kernel.width)];
            let products = products_kernel_name(index);
            graph.message(
                1,
                node(
                    "Conv",
                    &format!("conv_products_{index}"),
                    &[INPUT_NAME, products.as_str()],
                    &output_name(ConvOutputKind::Products, index),
                    Some(shape),
                ),
            );
            if kernel.mask.is_some() {
                let mask = mask_kernel_name(index);
                graph.message(
                    1,
                    node(
                        "Conv",
                        &format!("conv_image_sums_{index}"),
                        &[INPUT_NAME, mask.as_str()],
                        &output_name(ConvOutputKind::ImageSums, index),
                        Some(shape),
                    ),
                );
                graph.message(
                    1,
                    node(
                        "Conv",
                        &format!("conv_squared_sums_{index}"),
                        &[SQUARED_NAME, mask.as_str()],
                        &output_name(ConvOutputKind::SquaredSums, index),
                        Some(shape),
                    ),
                );
            }
        }
        graph.string(2, "template_conv");
        for (index, kernel) in self.scales.iter().enumerate() {
            let dims = [1, 1, i64::from(kernel.height), i64::from(kernel.width)];
            graph.message(
                5,
                initializer(&products_kernel_name(index), &dims, &kernel.products),
            );
            if let Some(mask) = &kernel.mask {
                graph.message(5, initializer(&mask_kernel_name(index), &dims, mask));
            }
        }
        graph.message(11, value_info(INPUT_NAME, &self.input_shape()));
        for (index, _, name) in self.outputs() {
            let shape = self
                .output_shape(index)
                .ok_or_else(|| graph_error(format!("scale {index} has no output shape")))?;
            graph.message(12, value_info(&name, &shape));
        }

        let mut opset = Writer::default();
        opset.string(1, "");
        opset.uint(2, OPSET_VERSION);
        let mut model = Writer::default();
        model.uint(1, IR_VERSION);
        model.string(2, PRODUCER_NAME);
        model.string(3, env!("CARGO_PKG_VERSION"));
        model.message(7, graph);
        model.message(8, opset);
        Ok(model.0)
    }
}

/// A protobuf message being written.
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn varint(&mut self, mut value: u64) {
        loop {
            let low = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.0.push(low);
                return;
            }
            self.0.push(low | 0x80);
        }
    }

    fn key(&mut self, field: u32, wire: u8) {
        self.varint((u64::from(field) << 3) | u64::from(wire));
    }

    fn uint(&mut self, field: u32, value: u64) {
        self.key(field, WIRE_VARINT);
        self.varint(value);
    }

    /// An `int64`: the two's complement as a varint.
    fn int(&mut self, field: u32, value: i64) {
        self.uint(field, value as u64);
    }

    fn bytes(&mut self, field: u32, data: &[u8]) {
        self.key(field, WIRE_BYTES);
        self.varint(data.len() as u64);
        self.0.extend_from_slice(data);
    }

    fn string(&mut self, field: u32, text: &str) {
        self.bytes(field, text.as_bytes());
    }

    fn message(&mut self, field: u32, message: Writer) {
        self.bytes(field, &message.0);
    }
}

/// A `NodeProto`; `kernel_shape` adds the attribute of that name.
fn node(
    op_type: &str,
    name: &str,
    inputs: &[&str],
    output: &str,
    kernel_shape: Option<[i64; 2]>,
) -> Writer {
    let mut node = Writer::default();
    for input in inputs {
        node.string(1, input);
    }
    node.string(2, output);
    node.string(3, name);
    node.string(4, op_type);
    if let Some(shape) = kernel_shape {
        let mut attribute = Writer::default();
        attribute.string(1, "kernel_shape");
        attribute.uint(20, ATTRIBUTE_INTS);
        for dim in shape {
            attribute.int(8, dim);
        }
        node.message(5, attribute);
    }
    node
}

/// A float `TensorProto` holding `values` as little-endian `raw_data`.
fn initializer(name: &str, dims: &[i64], values: &[f32]) -> Writer {
    let mut tensor = Writer::default();
    for dim in dims {
        tensor.int(1, *dim);
    }
    tensor.uint(2, ELEMENT_FLOAT);
    tensor.string(8, name);
    let raw: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    tensor.bytes(9, &raw);
    tensor
}

/// A `ValueInfoProto` of a float tensor of fixed `dims`.
fn value_info(name: &str, dims: &[i64]) -> Writer {
    let mut shape = Writer::default();
    for dim in dims {
        let mut dimension = Writer::default();
        dimension.int(1, *dim);
        shape.message(1, dimension);
    }
    let mut tensor = Writer::default();
    tensor.uint(1, ELEMENT_FLOAT);
    tensor.message(2, shape);
    let mut type_proto = Writer::default();
    type_proto.message(1, tensor);
    let mut info = Writer::default();
    info.string(1, name);
    info.message(2, type_proto);
    info
}

/// What a model holds, as read back from its bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDescription {
    pub ir_version: u64,
    pub producer_name: String,
    /// `(domain, version)` per opset import.
    pub opsets: Vec<(String, u64)>,
    pub inputs: Vec<ValueDescription>,
    pub outputs: Vec<ValueDescription>,
    pub initializers: Vec<InitializerDescription>,
    pub nodes: Vec<NodeDescription>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDescription {
    pub name: String,
    pub element_type: u64,
    pub dims: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InitializerDescription {
    pub name: String,
    pub data_type: u64,
    pub dims: Vec<i64>,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeDescription {
    pub name: String,
    pub op_type: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// The `kernel_shape` attribute, when the node has one.
    pub kernel_shape: Option<Vec<i64>>,
}

/// Reads a serialized `ModelProto` back (the members the generated graph writes).
pub fn describe_model(bytes: &[u8]) -> Result<ModelDescription, BenchError> {
    let mut model = ModelDescription {
        ir_version: 0,
        producer_name: String::new(),
        opsets: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        initializers: Vec::new(),
        nodes: Vec::new(),
    };
    let mut graphs = 0;
    for (field, value) in fields(bytes)? {
        match field {
            1 => model.ir_version = value.varint()?,
            2 => model.producer_name = value.text()?,
            7 => {
                graphs += 1;
                read_graph(value.bytes()?, &mut model)?;
            }
            8 => {
                let mut domain = String::new();
                let mut version = 0;
                for (field, value) in fields(value.bytes()?)? {
                    match field {
                        1 => domain = value.text()?,
                        2 => version = value.varint()?,
                        _ => {}
                    }
                }
                model.opsets.push((domain, version));
            }
            _ => {}
        }
    }
    if graphs != 1 {
        return Err(graph_error(format!(
            "the model holds {graphs} graphs, not one"
        )));
    }
    Ok(model)
}

fn read_graph(bytes: &[u8], model: &mut ModelDescription) -> Result<(), BenchError> {
    for (field, value) in fields(bytes)? {
        match field {
            1 => model.nodes.push(read_node(value.bytes()?)?),
            5 => model.initializers.push(read_tensor(value.bytes()?)?),
            11 => model.inputs.push(read_value_info(value.bytes()?)?),
            12 => model.outputs.push(read_value_info(value.bytes()?)?),
            _ => {}
        }
    }
    Ok(())
}

fn read_node(bytes: &[u8]) -> Result<NodeDescription, BenchError> {
    let mut node = NodeDescription {
        name: String::new(),
        op_type: String::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        kernel_shape: None,
    };
    for (field, value) in fields(bytes)? {
        match field {
            1 => node.inputs.push(value.text()?),
            2 => node.outputs.push(value.text()?),
            3 => node.name = value.text()?,
            4 => node.op_type = value.text()?,
            5 => {
                let mut name = String::new();
                let mut ints = Vec::new();
                for (field, value) in fields(value.bytes()?)? {
                    match field {
                        1 => name = value.text()?,
                        8 => value.push_ints(&mut ints)?,
                        _ => {}
                    }
                }
                if name == "kernel_shape" {
                    node.kernel_shape = Some(ints);
                }
            }
            _ => {}
        }
    }
    Ok(node)
}

fn read_tensor(bytes: &[u8]) -> Result<InitializerDescription, BenchError> {
    let mut tensor = InitializerDescription {
        name: String::new(),
        data_type: 0,
        dims: Vec::new(),
        values: Vec::new(),
    };
    for (field, value) in fields(bytes)? {
        match field {
            1 => value.push_ints(&mut tensor.dims)?,
            2 => tensor.data_type = value.varint()?,
            8 => tensor.name = value.text()?,
            9 => {
                let raw = value.bytes()?;
                if !raw.len().is_multiple_of(4) {
                    return Err(graph_error(
                        "a float initializer's raw_data is not a whole number of floats",
                    ));
                }
                let (floats, _) = raw.as_chunks::<4>();
                tensor
                    .values
                    .extend(floats.iter().map(|bytes| f32::from_le_bytes(*bytes)));
            }
            _ => {}
        }
    }
    Ok(tensor)
}

fn read_value_info(bytes: &[u8]) -> Result<ValueDescription, BenchError> {
    let mut info = ValueDescription {
        name: String::new(),
        element_type: 0,
        dims: Vec::new(),
    };
    for (field, value) in fields(bytes)? {
        match field {
            1 => info.name = value.text()?,
            2 => {
                for (field, value) in fields(value.bytes()?)? {
                    if field != 1 {
                        continue;
                    }
                    for (field, value) in fields(value.bytes()?)? {
                        match field {
                            1 => info.element_type = value.varint()?,
                            2 => {
                                for (field, value) in fields(value.bytes()?)? {
                                    if field != 1 {
                                        continue;
                                    }
                                    for (field, value) in fields(value.bytes()?)? {
                                        if field == 1 {
                                            info.dims.push(value.varint()? as i64);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(info)
}

/// One field of a protobuf message as it lies on the wire.
enum WireValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

impl<'a> WireValue<'a> {
    fn varint(&self) -> Result<u64, BenchError> {
        match self {
            Self::Varint(value) => Ok(*value),
            _ => Err(graph_error("a varint field has another wire type")),
        }
    }

    fn bytes(&self) -> Result<&'a [u8], BenchError> {
        match self {
            Self::Bytes(bytes) => Ok(*bytes),
            _ => Err(graph_error(
                "a length-delimited field has another wire type",
            )),
        }
    }

    fn text(&self) -> Result<String, BenchError> {
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|_| graph_error("a string field is not UTF-8"))
    }

    /// A repeated `int64`, packed or not.
    fn push_ints(&self, ints: &mut Vec<i64>) -> Result<(), BenchError> {
        match self {
            Self::Varint(value) => ints.push(*value as i64),
            Self::Bytes(bytes) => {
                let mut position = 0;
                while position < bytes.len() {
                    ints.push(read_varint(bytes, &mut position)? as i64);
                }
            }
            Self::Fixed => return Err(graph_error("an int64 field has a fixed wire type")),
        }
        Ok(())
    }
}

fn fields(bytes: &[u8]) -> Result<Vec<(u32, WireValue<'_>)>, BenchError> {
    let mut fields = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let key = read_varint(bytes, &mut position)?;
        let field = u32::try_from(key >> 3).map_err(|_| graph_error("a field number overflows"))?;
        let value = match (key & 0x7) as u8 {
            WIRE_VARINT => WireValue::Varint(read_varint(bytes, &mut position)?),
            WIRE_BYTES => {
                let length = usize::try_from(read_varint(bytes, &mut position)?)
                    .map_err(|_| graph_error("a length overflows"))?;
                let end = position
                    .checked_add(length)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| graph_error("a length-delimited field runs past the end"))?;
                let value = WireValue::Bytes(&bytes[position..end]);
                position = end;
                value
            }
            WIRE_FIXED64 => {
                position = skip(bytes, position, 8)?;
                WireValue::Fixed
            }
            WIRE_FIXED32 => {
                position = skip(bytes, position, 4)?;
                WireValue::Fixed
            }
            other => {
                return Err(graph_error(format!(
                    "field {field} has the unsupported wire type {other}"
                )));
            }
        };
        fields.push((field, value));
    }
    Ok(fields)
}

fn skip(bytes: &[u8], position: usize, length: usize) -> Result<usize, BenchError> {
    position
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| graph_error("a fixed-width field runs past the end"))
}

fn read_varint(bytes: &[u8], position: &mut usize) -> Result<u64, BenchError> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*position)
            .ok_or_else(|| graph_error("a varint runs past the end"))?;
        *position += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(graph_error("a varint is longer than ten bytes"))
}

fn graph_error(message: impl Into<String>) -> BenchError {
    BenchError::new(BenchCode::GraphInvalid, message)
}
