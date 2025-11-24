package crdtools

import (
	"testing"

	corev1 "k8s.io/api/core/v1"
)

func TestMergeLabels(t *testing.T) {
	lbl1 := map[string]string{"a": "1", "b": "2"}
	lbl2 := map[string]string{"b": "3", "c": "4"}

	ret := MergeLabels(lbl1, lbl2)

	if len(ret) != 3 {
		t.Errorf("expected 3 labels, got %d", len(ret))
	}

	if ret["a"] != "1" {
		t.Errorf("expected label 'a' to be '1', got '%s'", ret["a"])
	}

	if ret["b"] != "3" {
		t.Errorf("expected label 'b' to be '3', got '%s'", ret["b"])
	}

	if ret["c"] != "4" {
		t.Errorf("expected label 'c' to be '4', got '%s'", ret["c"])
	}
}

func TestMergeMounts(t *testing.T) {
	mnt1 := []corev1.VolumeMount{{Name: "a", MountPath: "/a"}}
	mnt2 := []corev1.VolumeMount{{Name: "b", MountPath: "/b"}}

	ret := MergeMounts(mnt1, mnt2)

	if len(ret) != 2 {
		t.Errorf("expected 2 mounts, got %d", len(ret))
	}

	if ret[0].Name != "a" {
		t.Errorf("expected mount 'a', got '%s'", ret[0].Name)
	}

	if ret[1].Name != "b" {
		t.Errorf("expected mount 'b', got '%s'", ret[1].Name)
	}
}

func TestMergeEnvFromSource(t *testing.T) {
	src1 := []corev1.EnvFromSource{{Prefix: "a"}}
	src2 := []corev1.EnvFromSource{{Prefix: "b"}}

	ret := MergeEnvFromSource(src1, src2)

	if len(ret) != 2 {
		t.Errorf("expected 2 sources, got %d", len(ret))
	}

	if ret[0].Prefix != "a" {
		t.Errorf("expected source 'a', got '%s'", ret[0].Prefix)
	}

	if ret[1].Prefix != "b" {
		t.Errorf("expected source 'b', got '%s'", ret[1].Prefix)
	}
}

func TestMergeEnvVar(t *testing.T) {
	env1 := []corev1.EnvVar{{Name: "a", Value: "1"}, {Name: "b", Value: "shadowed"}}
	env2 := []corev1.EnvVar{{Name: "b", Value: "2"}}

	ret := MergeEnvVar(env1, env2)

	if len(ret) != 2 {
		t.Errorf("expected 2 env vars, got %d", len(ret))
	}

	if ret[0].Name != "a" {
		t.Errorf("expected env var 'a', got '%s'", ret[0].Name)
	}

	if ret[1].Name != "b" {
		t.Errorf("expected env var 'b', got '%s'", ret[1].Name)
	}
}

func TestInt64Ptr(t *testing.T) {
	i := int64(42)
	ptr := Int64Ptr(i)

	if *ptr != i {
		t.Errorf("expected %d, got %d", i, *ptr)
	}
}

func TestInt32Ptr(t *testing.T) {
	i := int32(42)
	ptr := Int32Ptr(i)

	if *ptr != i {
		t.Errorf("expected %d, got %d", i, *ptr)
	}
}

func TestBoolPtr(t *testing.T) {
	b := true
	ptr := BoolPtr(b)

	if *ptr != b {
		t.Errorf("expected %t, got %t", b, *ptr)
	}
}
