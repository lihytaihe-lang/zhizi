import Foundation
import Accelerate
import Darwin

internal final class ZhijuFP32Tail: Sendable {
    enum TailError: Error {
        case invalidDimension
        case invalidVocabularyCount
        case invalidEpsilon
        case invalidGamma
        case invalidBeta
        case invalidWeights
        case invalidRows
        case invalidHidden
        case integerOverflow
        case nonFiniteValue
    }

    let dimension: Int
    let vocabularyCount: Int

    private let gamma: [Float]
    private let beta: [Float]
    private let weights: [Float]
    private let epsilon: Float

    init(gamma: [Float], beta: [Float], weights: [Float], dimension: Int, vocabularyCount: Int, epsilon: Float) throws {
        guard dimension > 0, dimension <= Int(Int32.max) else { throw TailError.invalidDimension }
        guard vocabularyCount >= 3, vocabularyCount <= Int(Int32.max) else { throw TailError.invalidVocabularyCount }
        guard epsilon.isFinite, epsilon > 0 else { throw TailError.invalidEpsilon }
        guard gamma.count == dimension, gamma.allSatisfy({ $0.isFinite }) else { throw TailError.invalidGamma }
        guard beta.count == dimension, beta.allSatisfy({ $0.isFinite }) else { throw TailError.invalidBeta }
        let (weightCount, overflow) = dimension.multipliedReportingOverflow(by: vocabularyCount)
        guard !overflow, weights.count == weightCount, weights.allSatisfy({ $0.isFinite }) else { throw TailError.invalidWeights }
        self.gamma = gamma
        self.beta = beta
        self.weights = weights
        self.dimension = dimension
        self.vocabularyCount = vocabularyCount
        self.epsilon = epsilon
    }

    func score(hidden: [Float], rows: Int) throws -> [Float] {
        guard rows >= 1, rows <= 8 else { throw TailError.invalidRows }
        let (hiddenCount, overflow) = rows.multipliedReportingOverflow(by: dimension)
        guard !overflow, hidden.count == hiddenCount, hidden.allSatisfy({ $0.isFinite }) else { throw TailError.invalidHidden }

        let d = dimension
        let v = vocabularyCount
        var normalized = [Float](repeating: 0, count: hiddenCount)
        var logits = [Float](repeating: 0, count: rows * v)
        var output = [Float](repeating: 0, count: rows * v)

        for row in 0..<rows {
            let base = row * d
            var sum: Float = 0
            for i in 0..<d { sum += hidden[base + i] }
            guard sum.isFinite else { throw TailError.nonFiniteValue }
            let mean = sum / Float(d)
            guard mean.isFinite else { throw TailError.nonFiniteValue }
            var centered = [Float](repeating: 0, count: d)
            var squared: Float = 0
            for i in 0..<d {
                let c = hidden[base + i] - mean
                guard c.isFinite else { throw TailError.nonFiniteValue }
                centered[i] = c
                squared += c * c
            }
            guard squared.isFinite else { throw TailError.nonFiniteValue }
            let variance = squared / Float(d)
            guard variance.isFinite else { throw TailError.nonFiniteValue }
            let scale = (variance + epsilon).squareRoot()
            guard scale.isFinite, scale > 0 else { throw TailError.nonFiniteValue }
            for i in 0..<d {
                let value = (centered[i] / scale) * gamma[i] + beta[i]
                guard value.isFinite else { throw TailError.nonFiniteValue }
                normalized[base + i] = value
            }
        }

        let m = Int32(rows)
        let k = Int32(d)
        let n = Int32(v)
        try normalized.withUnsafeMutableBufferPointer { normalizedBuffer in
            try weights.withUnsafeBufferPointer { weightBuffer in
                try logits.withUnsafeMutableBufferPointer { logitBuffer in
                    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans, m, n, k, 1.0,
                                normalizedBuffer.baseAddress, k,
                                weightBuffer.baseAddress, k,
                                0.0,
                                logitBuffer.baseAddress, n)
                }
            }
        }

        for row in 0..<rows {
            let base = row * v
            var maximum = logits[base]
            for i in 1..<v where logits[base + i] > maximum { maximum = logits[base + i] }
            guard maximum.isFinite else { throw TailError.nonFiniteValue }
            var sum: Float = 0
            for i in 0..<v {
                let shifted = logits[base + i] - maximum
                guard shifted.isFinite else { throw TailError.nonFiniteValue }
                let weight = expf(shifted)
                guard weight.isFinite else { throw TailError.nonFiniteValue }
                output[base + i] = shifted
                sum += weight
            }
            guard sum.isFinite, sum > 0 else { throw TailError.nonFiniteValue }
            let logSum = logf(sum)
            guard logSum.isFinite else { throw TailError.nonFiniteValue }
            for i in 0..<v {
                let value = output[base + i] - logSum
                guard value.isFinite else { throw TailError.nonFiniteValue }
                output[base + i] = value
            }
        }
        return output
    }
}
