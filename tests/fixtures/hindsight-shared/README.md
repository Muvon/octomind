These two tiny ONNX graphs return constant embeddings and logits. They exercise real
ORT session creation and cross-process Hindsight ownership without downloading model
weights. They use ONNX IR 8 and opset 13; the vocabulary contains only an unknown token.
The calibrated correction probability is 0.5. They are test fixtures, not trained models.
