-- ************************************************************
-- Title:	PUBLIC_DVD_GENCONDATA_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENCONDATA owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_GENCONDATA_202401010000.CSV
APPEND INTO TABLE GENCONDATA
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
GENCONID CHAR(20),
CONSTRAINTTYPE CHAR(2),
CONSTRAINTVALUE FLOAT EXTERNAL,
DESCRIPTION CHAR(256),
STATUS CHAR(8),
GENERICCONSTRAINTWEIGHT FLOAT EXTERNAL,
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
AUTHORISEDBY CHAR(15),
DYNAMICRHS FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
DISPATCH CHAR(1),
PREDISPATCH CHAR(1),
STPASA CHAR(1),
MTPASA CHAR(1),
IMPACT CHAR(64),
SOURCE CHAR(128),
LIMITTYPE CHAR(64),
REASON CHAR(256),
MODIFICATIONS CHAR(256),
ADDITIONALNOTES CHAR(256),
P5MIN_SCOPE_OVERRIDE CHAR(2),
LRC CHAR(1),
LOR CHAR(1),
FORCE_SCADA FLOAT EXTERNAL)
-- End of Script
