# WAFER Inference Models

## MNIST-8

Pre-trained MNIST digit recognition model for inference demo.

- **Source**: [ONNX Model Zoo](https://github.com/onnx/models/tree/main/validated/vision/classification/mnist)
- **Format**: ONNX
- **Input**: [1, 1, 28, 28] float32 tensor (batch, channels, height, width)
- **Output**: [1, 10] float32 tensor (logits for digits 0-9)
- **Size**: ~26KB

### Usage

This model is used by the `mnist-inference` plugin for digit recognition.
Input images should be 28x28 grayscale, normalized to [0, 1] range.

### License

Apache 2.0 (ONNX Model Zoo standard license)
