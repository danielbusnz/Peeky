#!/usr/bin/env python3
"""
Evaluate routelet intent classifier and generate confusion matrix.

Usage:
    cd Aegis
    pip install onnxruntime tokenizers numpy matplotlib seaborn scikit-learn
    python models/routelet/eval_routelet.py
"""

import json
import numpy as np
import matplotlib.pyplot as plt
import seaborn as sns
from pathlib import Path
from tokenizers import Tokenizer
import onnxruntime as ort
from sklearn.metrics import confusion_matrix, classification_report

MODEL_DIR = Path(__file__).parent

# Test examples with ground truth labels
# These should be unambiguous examples for each class
TEST_CASES = [
    # agent - multi-step tasks
    ("open youtube, search for lofi beats, and play the first result", "agent"),
    ("go to settings, find the privacy section, and disable tracking", "agent"),
    ("open my email, find the latest from amazon, and mark it as read", "agent"),
    ("search google for weather, then take a screenshot", "agent"),
    ("open spotify, go to my playlists, and play the first one", "agent"),

    # chat - general Q&A
    ("what is the capital of france", "chat"),
    ("how does photosynthesis work", "chat"),
    ("tell me a joke", "chat"),
    ("what's the weather like today", "chat"),
    ("who invented the telephone", "chat"),
    ("explain quantum computing", "chat"),
    ("what time is it in tokyo", "chat"),
    ("how do I make pasta carbonara", "chat"),

    # find_action - UI interaction
    ("where is the search bar", "find_action"),
    ("click the submit button", "find_action"),
    ("scroll down", "find_action"),
    ("find the settings icon", "find_action"),
    ("click on the red X", "find_action"),
    ("where is the menu", "find_action"),
    ("tap the play button", "find_action"),
    ("point to the login link", "find_action"),
    ("type hello into the text box", "find_action"),

    # integration - service calls
    ("play despacito on spotify", "integration"),
    ("check my gmail", "integration"),
    ("show my github pull requests", "integration"),
    ("play some jazz music", "integration"),
    ("what's playing right now", "integration"),
    ("skip this song", "integration"),
    ("pause the music", "integration"),
    ("show my unread emails", "integration"),
    ("play the next track", "integration"),

    # memory - store/recall facts
    ("remember my wifi password is hunter2", "memory"),
    ("what's my favorite color", "memory"),
    ("remember that I'm allergic to peanuts", "memory"),
    ("what did I tell you my name was", "memory"),
    ("remember my birthday is march 5th", "memory"),
    ("do you remember my home address", "memory"),
    ("my favorite food is pizza", "memory"),

    # none - ambiguous or out of scope
    ("hmm", "none"),
    ("uh", "none"),
    ("never mind", "none"),
    ("cancel", "none"),
    ("stop", "none"),
    ("wait", "none"),
]


def load_model():
    """Load ONNX model, tokenizer, and head weights."""
    # Load ONNX model
    onnx_path = MODEL_DIR / "embedder.onnx"
    session = ort.InferenceSession(str(onnx_path))

    # Load tokenizer
    tok_path = MODEL_DIR / "tokenizer.json"
    tokenizer = Tokenizer.from_file(str(tok_path))

    # Load head weights
    head_path = MODEL_DIR / "head.json"
    with open(head_path) as f:
        head = json.load(f)

    coef = np.array(head["coef"], dtype=np.float32)
    intercept = np.array(head["intercept"], dtype=np.float32)
    labels = head["labels"]
    temperature = head.get("temperature", 1.0)

    return session, tokenizer, coef, intercept, labels, temperature


def embed(session, tokenizer, text: str) -> np.ndarray:
    """Run text through tokenizer and ONNX embedder."""
    encoding = tokenizer.encode(text)

    ids = np.array([encoding.ids], dtype=np.int64)
    mask = np.array([encoding.attention_mask], dtype=np.int64)
    type_ids = np.array([encoding.type_ids], dtype=np.int64)

    outputs = session.run(
        None,
        {
            "input_ids": ids,
            "attention_mask": mask,
            "token_type_ids": type_ids,
        }
    )

    return outputs[0].flatten()


def predict(embedding, coef, intercept, labels, temperature):
    """Run logistic regression head with temperature scaling."""
    logits = coef @ embedding + intercept
    logits = logits / temperature

    # Stable softmax
    exp_logits = np.exp(logits - np.max(logits))
    probs = exp_logits / np.sum(exp_logits)

    best_idx = np.argmax(probs)
    return labels[best_idx], probs[best_idx]


def main():
    print("Loading model...")
    session, tokenizer, coef, intercept, labels, temperature = load_model()
    print(f"Labels: {labels}")
    print(f"Temperature: {temperature}\n")

    y_true = []
    y_pred = []

    print("Running evaluation...")
    for text, true_label in TEST_CASES:
        embedding = embed(session, tokenizer, text)
        pred_label, confidence = predict(embedding, coef, intercept, labels, temperature)
        y_true.append(true_label)
        y_pred.append(pred_label)

        marker = "x" if pred_label != true_label else " "
        print(f"[{marker}] {true_label:12} -> {pred_label:12} ({confidence:.2f})  {text[:50]}")

    # Generate confusion matrix
    print("\n" + "="*60)
    print("Classification Report:")
    print("="*60)
    print(classification_report(y_true, y_pred, labels=labels, zero_division=0))

    # Plot confusion matrix
    cm = confusion_matrix(y_true, y_pred, labels=labels)

    plt.figure(figsize=(10, 8))
    sns.heatmap(
        cm,
        annot=True,
        fmt="d",
        cmap="Blues",
        xticklabels=labels,
        yticklabels=labels,
    )
    plt.xlabel("Predicted")
    plt.ylabel("True")
    plt.title("Routelet Intent Classifier - Confusion Matrix")
    plt.tight_layout()

    output_path = MODEL_DIR / "confusion_matrix.png"
    plt.savefig(output_path, dpi=150)
    print(f"\nConfusion matrix saved to: {output_path}")


if __name__ == "__main__":
    main()
