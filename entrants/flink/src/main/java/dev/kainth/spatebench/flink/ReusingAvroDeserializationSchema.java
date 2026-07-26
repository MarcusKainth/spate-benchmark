package dev.kainth.spatebench.flink;

import io.confluent.kafka.schemaregistry.client.CachedSchemaRegistryClient;
import org.apache.avro.Schema;
import org.apache.avro.generic.GenericData;
import org.apache.avro.generic.GenericDatumReader;
import org.apache.avro.generic.GenericRecord;
import org.apache.avro.io.BinaryDecoder;
import org.apache.avro.io.DecoderFactory;
import org.apache.flink.api.common.serialization.DeserializationSchema;
import org.apache.flink.api.common.typeinfo.TypeInformation;
import org.apache.flink.formats.avro.SchemaCoder;
import org.apache.flink.formats.avro.registry.confluent.ConfluentSchemaRegistryCoder;
import org.apache.flink.formats.avro.typeutils.GenericRecordAvroTypeInfo;
import org.apache.flink.formats.avro.utils.MutableByteArrayInputStream;

import java.io.IOException;

/**
 * SECONDARY ARM ONLY — selected by {@code DESER=reusing}, never the headline.
 *
 * <p>This exists to put a number on one specific cost in Flink's shipped
 * {@code ConfluentRegistryAvroDeserializationSchema}: it re-uses the
 * {@code BinaryDecoder} but calls {@code datumReader.read(null, decoder)}, so every
 * message allocates a fresh {@code GenericData.Record} — and, through it, a fresh
 * {@code Utf8} plus backing {@code byte[]} for each of the ~44 string fields in a
 * 20-event batch. It also calls {@code setSchema}/{@code setExpected} on every
 * message, which invalidates the datum reader's resolver and costs two identity-map
 * lookups per message.
 *
 * <p>Both are avoidable, and this class is the ~20 lines that avoid them: pass the
 * previous record back as the reuse argument, and only re-point the reader when the
 * writer schema identity actually changes. Everything else is deliberately
 * identical to Flink's implementation — the same {@link ConfluentSchemaRegistryCoder},
 * the same {@link MutableByteArrayInputStream}, the same single reused
 * {@code BinaryDecoder} — so the measured delta is attributable to record reuse
 * alone and not to a rewritten decoder.
 *
 * <p>The primary arm does <em>not</em> use this, because
 * {@code methodology/} rule 1 forbids hand-writing a competitor's
 * internals: doing so would measure our Java rather than Flink. Publishing the
 * delta as a labelled secondary is how the same rule wants a shipped default's cost
 * quantified.
 *
 * <p>Caveat a reviewer should know: reuse is only safe because the row is fully
 * copied out inside {@code FlattenTierA}/{@code FlattenTierB} before the next
 * message is decoded, which holds here because source, flatMap and sink share one
 * operator chain. It would be wrong in a job that buffered {@code GenericRecord}s.
 */
public final class ReusingAvroDeserializationSchema
        implements DeserializationSchema<GenericRecord> {

    private static final long serialVersionUID = 1L;

    /** Matches Flink's own default identity-map capacity for the registry client. */
    private static final int IDENTITY_MAP_CAPACITY = 1000;

    private final String readerSchemaJson;
    private final String registryUrl;

    private transient Schema readerSchema;
    private transient SchemaCoder coder;
    private transient GenericDatumReader<GenericRecord> datumReader;
    private transient MutableByteArrayInputStream inputStream;
    private transient BinaryDecoder decoder;
    private transient Schema lastWriterSchema;
    private transient GenericRecord reuse;

    ReusingAvroDeserializationSchema(String readerSchemaJson, String registryUrl) {
        this.readerSchemaJson = readerSchemaJson;
        this.registryUrl = registryUrl;
    }

    @Override
    public void open(InitializationContext context) {
        readerSchema = SensorBatchSchema.parse(readerSchemaJson);
        coder = new ConfluentSchemaRegistryCoder(
                new CachedSchemaRegistryClient(registryUrl, IDENTITY_MAP_CAPACITY));
        ClassLoader cl = Thread.currentThread().getContextClassLoader();
        datumReader = new GenericDatumReader<>(null, readerSchema, new GenericData(cl));
        inputStream = new MutableByteArrayInputStream();
        decoder = DecoderFactory.get().binaryDecoder(inputStream, null);
        lastWriterSchema = null;
        reuse = null;
    }

    @Override
    public GenericRecord deserialize(byte[] message) throws IOException {
        if (message == null) {
            return null;
        }
        inputStream.setBuffer(message);
        Schema writerSchema = coder.readSchema(inputStream);
        if (writerSchema != lastWriterSchema) {
            // Identity comparison is enough: the registry client caches Schema
            // instances per id, so a steady-state stream takes this branch once.
            datumReader.setSchema(writerSchema);
            datumReader.setExpected(readerSchema);
            lastWriterSchema = writerSchema;
            reuse = null;
        }
        reuse = datumReader.read(reuse, decoder);
        return reuse;
    }

    @Override
    public boolean isEndOfStream(GenericRecord nextElement) {
        return false;
    }

    @Override
    public TypeInformation<GenericRecord> getProducedType() {
        return new GenericRecordAvroTypeInfo(SensorBatchSchema.parse(readerSchemaJson));
    }
}
